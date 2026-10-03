//! Project-scoped MCP servers: the trust gate and the per-definition approval —
//! `project-server-trust.ts` (242 lines, new at `5d645df` / #681 and refined six times since).
//!
//! # Why this module exists
//!
//! `.mcp.json` and `.cyrup/mcp.json` are read **from the checkout**. An entry may name an arbitrary
//! stdio `command`, and an `!`-prefixed `env` value runs a shell command at connect
//! ([`crate::secrets`]). Cloning a repository and starting cyrup in it was therefore enough to run
//! attacker-chosen code with the user's permissions. `MCP-096` recorded that as an open decision —
//! *"upstream applies no gate; cyrup's loader skips the whole project layer for an untrusted
//! project … Recommendation: gate (option b), record the divergence"* — and upstream has now closed
//! it in the same direction, with a concrete mechanism. `MCP-591` is the port.
//!
//! # Two independent gates, not one
//!
//! 1. **Project trust** — `ctx.isProjectTrusted()`. An untrusted project's servers are blocked
//!    outright, with no prompt: there is nobody to usefully ask, because the user has not yet said
//!    they trust this checkout at all.
//! 2. **Per-definition approval** — a trusted project's servers are each approved once, and the
//!    approval is keyed on a hash of the definition, so **editing an approved server re-prompts**.
//!    `settings.projectServers: "allow"` waives the prompt for a *non-interactive* session only.
//!
//! A server that fails either gate is not removed: it is marked `disabled: true` and recorded in
//! [`ProjectTrustResult::blocked_servers`] with one of three reasons, so `/mcp status`, the panel
//! and an agent's blocked tool call can all say *why* rather than reporting a server that silently
//! does not exist. That is the difference from cyrup's pre-MCP-591 behaviour, where
//! `ConfigContext::source_contributes` made the whole project layer vanish.
//!
//! # What is deliberately not here
//!
//! `ConfigContext::source_contributes` (MCP-096's partial option (b)) survives, and is **inert in
//! production**: `project_trusted` defaults to `true` and nothing outside this crate's tests sets
//! it. It is left standing because it is a narrower, strictly-more-conservative gate than this
//! module's and removing it is not this unit's change; the gate that runs is this one.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;

use crate::config::{McpConfig, ProjectServerPolicy, ProjectServerSource, ServerEntry};
use crate::dirs::McpDirs;

/// `APPROVALS_VERSION` (`project-server-trust.ts:9`). A store written by a newer version is
/// discarded, not migrated.
pub const APPROVALS_VERSION: u8 = 1;

/// `APPROVALS_FILE` (`:10`) — `<agent_dir>/mcp-project-approvals.json`.
pub const APPROVALS_FILE: &str = "mcp-project-approvals.json";

/// `DENY_PROJECT_SERVER` (`:11`).
pub const DENY_PROJECT_SERVER: &str = "Don't allow";

/// `ALLOW_PROJECT_SERVER` (`:12`).
pub const ALLOW_PROJECT_SERVER: &str = "Allow";

/// `PROJECT_SERVER_CHOICES` (`:14`) — **deny first**, and that order is the hardening `67fcdf9`
/// (#797) landed as the last functional commit before v5.0.0.
///
/// Upstream's comment is the reason, verbatim: *"Pi preselects the first option, so a stray Enter
/// denies."* Reversing these two labels would turn the dialog into a one-keystroke approval of
/// arbitrary code execution, which is why the constant exists instead of an inline array.
pub const PROJECT_SERVER_CHOICES: [&str; 2] = [DENY_PROJECT_SERVER, ALLOW_PROJECT_SERVER];

/// `ProjectServerBlockReason` (`types.ts:61`) — why a project server was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectServerBlockReason {
    /// The project itself is not trusted, so no prompt was offered.
    Untrusted,
    /// Trusted, but the session cannot ask: no interactive UI, and the policy is `ask`.
    ApprovalRequired,
    /// Trusted and asked, and the user said no — including by pressing Enter on the preselected
    /// `Don't allow`, and by dismissing the dialog.
    Denied,
}

/// `ProjectServerBlock` (`types.ts:63`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectServerBlock {
    /// Which gate refused it.
    pub reason: ProjectServerBlockReason,
    /// The project file that asked for the server.
    pub source: ProjectServerSource,
}

/// `ProjectTrustResult` (`project-server-trust.ts:30`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectTrustResult {
    /// The config to run, with every blocked server's entry carrying `disabled: true`.
    pub config: McpConfig,
    /// One entry per blocked server, in the order the project set named them.
    pub blocked_servers: IndexMap<String, ProjectServerBlock>,
}

/// `describeProjectServerBlock(reason)` (`:35`) — the three agent-facing strings, verbatim.
///
/// Each one names the *action that clears it*, which is why they are not interchangeable: an
/// untrusted project needs the project trusted, a non-interactive session needs either an
/// interactive one or the user-global setting, and a denial needs a reload in a session that can
/// ask again.
#[must_use]
pub fn describe_project_server_block(reason: ProjectServerBlockReason) -> &'static str {
    match reason {
        ProjectServerBlockReason::Untrusted => {
            "blocked by project trust — trust the project to review and approve this server"
        }
        ProjectServerBlockReason::ApprovalRequired => {
            "blocked: project server approval required — approve it in a trusted interactive session or set user-global settings.projectServers to \"allow\""
        }
        ProjectServerBlockReason::Denied => {
            "blocked: project server approval denied — reload in a trusted interactive session to approve it"
        }
    }
}

/// `disabledServerReason(blocked, name)` (`:46`) — the block reason when there is one, and the
/// ordinary disabled sentence otherwise.
///
/// Every "this server is disabled" message in the adapter routes through here, so a blocked project
/// server never tells the user to run `/mcp enable`, which would not help: the entry is not
/// disabled in any file they can edit.
#[must_use]
pub fn disabled_server_reason(
    blocked: Option<&IndexMap<String, ProjectServerBlock>>,
    name: &str,
) -> String {
    match blocked.and_then(|blocked| blocked.get(name)) {
        Some(block) => describe_project_server_block(block.reason).to_string(),
        None => format!("disabled. Run /mcp enable {name} and /reload to enable it."),
    }
}

/// `hasProjectServerDefinitions(config)` (`:64`).
#[must_use]
pub fn has_project_server_definitions(
    project_servers: &IndexMap<String, ProjectServerSource>,
) -> bool {
    !project_servers.is_empty()
}

/// `hashProjectServerDefinition(definition)` (`:77`) — SHA-256 over a canonicalised definition.
///
/// `canonicalize` is a recursive key sort with `undefined` members dropped, and
/// `JSON.stringify` of the result is what is hashed. Both halves fall out of the types here:
/// [`ServerEntry`] skips every `None` field on serialize, and `serde_json::Map` is a `BTreeMap`
/// under this workspace's feature set (`preserve_order` is off), so converting to a
/// [`serde_json::Value`] sorts every object's keys at every depth. Arrays keep their order, as
/// upstream's `value.map(canonicalize)` does.
///
/// # The key sort is explicit, because `serde_json::Map` here is NOT sorted
///
/// This workspace declares `serde_json/preserve_order` (root `Cargo.toml`), so a `Map` is an
/// `IndexMap` and `to_value` on a struct preserves **declaration** order. Leaning on the map type
/// to sort — as a comment elsewhere in this crate still claims it does — would have made the hash
/// depend on `ServerEntry`'s field order, so reordering two fields in the struct would have
/// silently invalidated every stored approval. [`canonicalize_json`] does the sort, at every depth,
/// and the test for it is `the_definition_hash_is_order_insensitive_and_field_sensitive`.
///
/// # This hash is cyrup's own, end to end
///
/// Unlike [`crate::dirs::compute_server_hash`], nothing outside this crate reads it: it is written
/// into cyrup's own approval store and compared against a value this same function produced. What
/// it owes is determinism and *sensitivity* — editing any field of an approved definition must
/// change it — not byte-compatibility with pi, whose store is a different file. Two places where
/// it could differ from pi's are therefore recorded rather than engineered around: upstream sorts
/// keys with `localeCompare` where this sorts by code point (identical for the ASCII field names,
/// potentially different for a non-ASCII `env` member name), and a whole-valued float renders as
/// `2500.0` here where `JSON.stringify` writes `2500`.
#[must_use]
pub fn hash_project_server_definition(definition: &ServerEntry) -> String {
    crate::dirs::hex_sha256(canonical_json(definition).as_bytes())
}

/// `canonicalize(value)` (`project-server-trust.ts:68`) — arrays keep their order, objects have
/// their keys sorted, and a member whose value is `undefined` is dropped.
///
/// The `undefined` clause is already satisfied upstream of this by `ServerEntry`'s
/// `skip_serializing_if = "Option::is_none"`, so no `null` member can reach here from a definition;
/// the arm is written anyway because `canonicalize` is defined over arbitrary JSON and a reader
/// comparing the two should not have to work out which half is missing. Note it drops `undefined`
/// and **keeps** `null`, which is upstream's `entry !== undefined` exactly.
fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_json).collect())
        }
        serde_json::Value::Object(members) => {
            let mut sorted: Vec<(&String, &serde_json::Value)> = members.iter().collect();
            sorted.sort_by_key(|(key, _)| *key);
            serde_json::Value::Object(
                sorted
                    .into_iter()
                    .map(|(key, member)| (key.clone(), canonicalize_json(member)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// `canonicalProjectRoot(cwd)` (`:81`) — `realpathSync(cwd)`, falling back to `resolve(cwd)`.
///
/// The fallback is not cosmetic: a cwd that has been deleted under the session still has to produce
/// a stable key, or every approval for it would be unfindable.
#[must_use]
pub fn canonical_project_root(cwd: &Path) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| resolve_lexical(cwd))
}

/// `resolve(cwd)` — lexical normalisation against the process cwd, with no filesystem access.
///
/// `std::fs::canonicalize` is the `realpathSync` of the pair and has already failed by the time
/// this runs, so this must not touch the disk either.
fn resolve_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `projectApprovalScope(cwd)` (`:100`) — the key an approval is stored under.
///
/// Upstream's doc comment is the specification and is reproduced in full because every clause of it
/// is a defence:
///
/// > Git worktrees share approvals per repository, keyed by the same relative path. A `.git` file
/// > counts only when it is a regular file (not a symlink to a real worktree's) and git's admin
/// > entry for it links back to it; otherwise a directory could claim another checkout's approvals.
/// > Worktrees of a regular checkout use that checkout's path, so its existing approvals still
/// > apply. Bare repositories (even one stored as a `.git` folder, whose parent is not a checkout)
/// > and `--separate-git-dir` ones get a `git-dir:` key, which no canonical path equals, so an
/// > admin entry planted in a checkout's tracked files cannot borrow that checkout's approvals.
/// > A `--separate-git-dir` main checkout keeps its own path: git records no link back to it.
///
/// So: approving a server in one worktree approves it in its siblings — which is the point, because
/// they are the same repository — while a `.git` file a *repository* could ship cannot make a
/// checkout inherit someone else's approvals.
#[must_use]
pub fn project_approval_scope(cwd: &Path) -> String {
    let root = canonical_project_root(cwd);
    let mut dir: &Path = root.as_path();
    loop {
        let dot_git = dir.join(".git");
        if dot_git.exists() {
            return match linked_worktree_repo_scope(&dot_git) {
                // `join(repoScope, relative(dir, root))` — the SAME relative path in every
                // worktree, so two worktrees of one repository agree.
                Some(repo_scope) => {
                    let relative = root.strip_prefix(dir).unwrap_or(Path::new(""));
                    // `join("")` appends a separator in Rust where node's `join(x, "")` does not,
                    // and the difference is a DIFFERENT KEY: the main worktree of a checkout has an
                    // empty relative path, so without this guard it stored its approvals under
                    // `<repo>/` while every other reader looked under `<repo>`.
                    if relative.as_os_str().is_empty() {
                        repo_scope
                    } else {
                        Path::new(&repo_scope)
                            .join(relative)
                            .to_string_lossy()
                            .into_owned()
                    }
                }
                None => root.to_string_lossy().into_owned(),
            };
        }
        match dir.parent() {
            // `if (dirname(dir) === dir) return root` — the filesystem root.
            Some(parent) if parent != dir => dir = parent,
            _ => return root.to_string_lossy().into_owned(),
        }
    }
}

/// `linkedWorktreeRepoScope(dotGit)` (`:112`) — `Some(scope)` when `dotGit` is a verified linked
/// worktree pointer, `None` for anything else (including every failure).
fn linked_worktree_repo_scope(dot_git: &Path) -> Option<String> {
    // `lstatSync(dotGit).isFile()` — a SYMLINK to a real worktree's `.git` file is refused, which
    // is why this is `symlink_metadata` and not `metadata`.
    if !std::fs::symlink_metadata(dot_git).ok()?.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(dot_git).ok()?;
    let pointer = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir: "))?
        .trim();
    if pointer.is_empty() {
        return None;
    }
    let admin_dir = std::fs::canonicalize(dot_git.parent()?.join(pointer)).ok()?;
    // The BACK-LINK check. Git writes `<admin>/gitdir` pointing at the worktree's own `.git` file;
    // without this test any directory could ship a `.git` file aimed at another checkout's admin
    // entry and inherit that checkout's approvals.
    let back_link = std::fs::read_to_string(admin_dir.join("gitdir")).ok()?;
    let back_target = std::fs::canonicalize(admin_dir.join(back_link.trim())).ok()?;
    if back_target != std::fs::canonicalize(dot_git).ok()? {
        return None;
    }
    // `<repo>/.git/worktrees/<name>` → `<repo>/.git`, then `<repo>`.
    let common_dir = admin_dir.parent()?.parent()?;
    let bare = std::fs::read_to_string(common_dir.join("config"))
        .ok()
        .is_some_and(|config| config_says_bare(&config));
    if common_dir.file_name().is_some_and(|name| name == ".git") && !bare {
        // A regular checkout: use ITS path, so approvals already stored for the main worktree
        // apply to the linked ones.
        return Some(common_dir.parent()?.to_string_lossy().into_owned());
    }
    // A bare repository, or `--separate-git-dir`. `git-dir:` prefixed, and no canonical path can
    // ever equal that, so this scope is unreachable by planting a pointer in a checkout.
    Some(format!("git-dir:{}", common_dir.to_string_lossy()))
}

/// `/^\s*bare\s*=\s*(true|yes|on|1)\s*$/im` over a git config file.
///
/// Hand-rolled rather than a regex dependency, and every flag of the pattern is reproduced: the
/// four truthy spellings are git's own (a `bare = yes` repository is bare and must not be mistaken
/// for a checkout), and the `i` is **not** optional — git config keys are case-insensitive, so a
/// `BARE = true` written by hand or by another tool has to count.
fn config_says_bare(config: &str) -> bool {
    config.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        let Some(value) = line.strip_prefix("bare") else {
            return false;
        };
        let Some(value) = value.trim_start().strip_prefix('=') else {
            return false;
        };
        matches!(value.trim(), "true" | "yes" | "on" | "1")
    })
}

/// One `approvals[]` record (`:18`).
///
/// `camelCase` on the wire, because the file is `JSON.stringify`'d from a TypeScript interface and
/// the four key names are part of its format. A snake_case spelling here would write a store this
/// same function could not read back, which is how the test for a malformed record caught it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRecord {
    /// [`project_approval_scope`]'s answer at the time of approval.
    pub project_root: String,
    /// The `mcpServers` key.
    pub server_name: String,
    /// [`hash_project_server_definition`] of the definition that was approved.
    pub definition_hash: String,
    /// `new Date().toISOString()`.
    pub approved_at: String,
}

/// The whole store (`:25`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApprovalStore {
    /// Always [`APPROVALS_VERSION`]; any other value makes the file invalid.
    pub version: u8,
    /// Every stored approval, newest last.
    pub approvals: Vec<ApprovalRecord>,
}

impl Default for ApprovalStore {
    fn default() -> Self {
        Self {
            version: APPROVALS_VERSION,
            approvals: Vec::new(),
        }
    }
}

/// `approvalPath()` (`:128`) — `getAgentPath(APPROVALS_FILE)`.
#[must_use]
pub fn approval_path(dirs: &McpDirs) -> PathBuf {
    dirs.agent_path(APPROVALS_FILE)
}

/// `loadApprovals()` (`:132`) — and it **never fails**, which is the whole point.
///
/// A missing store is an empty one. A store that is unreadable, unparseable, of the wrong version
/// or not an array warns once and is likewise treated as empty: a corrupt file must not block every
/// project server, it must make them all ask again. Records that are individually malformed are
/// filtered out and the rest survive.
///
/// Failing *open* here would be the dangerous direction; failing to an empty store means the user
/// is asked again, which is the safe one.
#[must_use]
pub fn load_approvals(dirs: &McpDirs) -> ApprovalStore {
    let path = approval_path(dirs);
    if !path.exists() {
        return ApprovalStore::default();
    }
    let invalid = |detail: &dyn std::fmt::Display| {
        tracing::warn!(
            "MCP: ignoring invalid project-server approval store {}: {detail}",
            path.display()
        );
        ApprovalStore::default()
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => return invalid(&error),
    };
    // `JSON.parse`, not the JSONC reader: this is the adapter's own store, never hand-edited, and
    // upstream parses it strictly.
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(parsed) => parsed,
        Err(error) => return invalid(&error),
    };
    if parsed.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(APPROVALS_VERSION))
    {
        return invalid(&"invalid format");
    }
    let Some(records) = parsed
        .get("approvals")
        .and_then(serde_json::Value::as_array)
    else {
        return invalid(&"invalid format");
    };
    ApprovalStore {
        version: APPROVALS_VERSION,
        // Per-record filtering, not whole-file rejection: upstream's `.filter(…is ApprovalRecord)`
        // keeps every record whose four fields are strings and drops the others silently.
        approvals: records
            .iter()
            .filter_map(|record| serde_json::from_value::<ApprovalRecord>(record.clone()).ok())
            .collect(),
    }
}

/// `saveApproval(record)` (`:149`) — replace any record for the same `(projectRoot, serverName)`,
/// append, and write the store atomically with `0600`.
///
/// The mode is not decoration: the file records which repositories the user has allowed to run
/// commands, so another local account must not be able to append to it. The directory is created
/// `0700` for the same reason, and the rename is what makes a concurrent reader see either the old
/// store or the new one.
///
/// # Errors
///
/// [`crate::errors::McpError::Io`] for any filesystem failure, carrying the path.
pub fn save_approval(dirs: &McpDirs, record: ApprovalRecord) -> crate::errors::McpResult<()> {
    let path = approval_path(dirs);
    let mut store = load_approvals(dirs);
    store.approvals.retain(|entry| {
        entry.project_root != record.project_root || entry.server_name != record.server_name
    });
    store.approvals.push(record);

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| crate::errors::McpError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        set_mode(parent, 0o700);
    }
    let serialized = serde_json::to_string_pretty(&store)
        .map(|text| format!("{text}\n"))
        .map_err(|error| {
            crate::errors::McpError::Config(format!(
                "Failed to serialize the project-server approval store: {error}"
            ))
        })?;
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, serialized).map_err(|source| crate::errors::McpError::Io {
        path: tmp.clone(),
        source,
    })?;
    set_mode(&tmp, 0o600);
    if let Err(source) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::errors::McpError::Io {
            path: path.clone(),
            source,
        });
    }
    set_mode(&path, 0o600);
    Ok(())
}

/// `chmodSync(path, mode)` on unix, a no-op elsewhere (`if (process.platform !== "win32")`).
fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// `approveProjectServer(cwd, serverName, definition)` (`:163`).
///
/// # Errors
///
/// Whatever [`save_approval`] returns.
pub fn approve_project_server(
    dirs: &McpDirs,
    cwd: &Path,
    server_name: &str,
    definition: &ServerEntry,
) -> crate::errors::McpResult<()> {
    save_approval(
        dirs,
        ApprovalRecord {
            project_root: project_approval_scope(cwd),
            server_name: server_name.to_string(),
            definition_hash: hash_project_server_definition(definition),
            approved_at: now_iso8601(),
        },
    )
}

/// `new Date().toISOString()` — `YYYY-MM-DDTHH:MM:SS.mmmZ`, always UTC, always three fractional
/// digits.
///
/// Stored for the user's benefit and never parsed back, so the format is the contract and the value
/// is not: nothing compares two `approvedAt`s.
fn now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis();
    let secs = i64::try_from(millis / 1000).unwrap_or(0);
    let sub_millis = u32::try_from(millis % 1000).unwrap_or(0);
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub_millis:03}Z")
}

/// Days-from-civil's inverse — Howard Hinnant's `civil_from_days`, which is exact for every day in
/// the proleptic Gregorian calendar and needs no date dependency.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
        u32::try_from(rem / 3600).unwrap_or(0),
        u32::try_from((rem % 3600) / 60).unwrap_or(0),
        u32::try_from(rem % 60).unwrap_or(0),
    )
}

/// `describeServer(definition)` (`:172`) — the endpoint line of the approval prompt.
///
/// `command` wins over `url`, and each `command`/`args` element is `JSON.stringify`'d, so an
/// argument containing a space or a quote cannot be made to look like two arguments in the dialog
/// the user is about to approve.
#[must_use]
pub fn describe_server(definition: &ServerEntry) -> String {
    if let Some(command) = definition.command.as_deref() {
        let mut parts = vec![json_quote(command)];
        parts.extend(definition.args.iter().flatten().map(|arg| json_quote(arg)));
        return parts.join(" ");
    }
    if let Some(url) = definition.url.as_deref() {
        return url.to_string();
    }
    "(no command or endpoint)".to_string()
}

/// `JSON.stringify(value)` for a string.
fn json_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

/// What [`apply_project_server_trust`] needs from the session — upstream's
/// `Pick<ExtensionContext, "cwd" | "hasUI" | "mode" | "ui" | "isProjectTrusted">`.
pub struct ProjectTrustContext<'a> {
    /// `ctx.cwd` — the approval scope's input.
    pub cwd: &'a Path,
    /// `<agent_dir>` — where the approval store lives.
    pub dirs: &'a McpDirs,
    /// `ctx.isProjectTrusted()`. Upstream wraps the call in a `try/catch` that answers **false**,
    /// so a host that cannot answer is treated as untrusted; the caller does the same by passing
    /// `false`.
    pub project_trusted: bool,
    /// `ctx.hasUI`. Distinguishes "cannot ask" from "asked and was refused", which are two
    /// different block reasons and two different remedies.
    pub has_ui: bool,
    /// The merged `settings.projectServers`.
    pub policy: ProjectServerPolicy,
    /// The dialog surface. `None` whenever [`Self::has_ui`] is false, and the two must agree: a
    /// session with a UI but no handle cannot prompt and so cannot approve.
    ///
    /// [`crate::owner::McpDialog`] rather than the raw services handle, so the prompt takes the
    /// human-interaction lock: a project-server approval must queue behind an in-flight permission
    /// prompt rather than painting over it.
    pub ui: Option<&'a crate::owner::McpDialog>,
}

/// `applyProjectServerTrust(loaded, ctx)` (`:181`) — the gate.
///
/// Returns the config to run and the blocked set. A server is **skipped entirely** when it has no
/// definition in the merged table (a higher-precedence source replaced it, so it is no longer the
/// project's) or when it is already disabled — `7dc3d28` (#688): there is no point asking the user
/// to approve a server that is not going to start either way, and asking would train them to
/// approve things reflexively.
///
/// The admission test is one expression upstream and is kept as one here:
/// `projectTrusted && (approved || (!ctx.hasUI && policy === "allow"))`. Note that the `allow`
/// policy waives the prompt only for a **non-interactive** session: where a human is present,
/// upstream asks anyway.
pub async fn apply_project_server_trust(
    config: &McpConfig,
    project_servers: &IndexMap<String, ProjectServerSource>,
    ctx: &ProjectTrustContext<'_>,
) -> ProjectTrustResult {
    let mut result = ProjectTrustResult {
        config: config.clone(),
        blocked_servers: IndexMap::new(),
    };
    if project_servers.is_empty() {
        return result;
    }

    let project_root = project_approval_scope(ctx.cwd);
    let approvals = load_approvals(ctx.dirs);

    for (name, source) in project_servers {
        let Some(definition) = result.config.mcp_servers.get(name).cloned() else {
            continue;
        };
        if definition.is_disabled() {
            continue;
        }
        let definition_hash = hash_project_server_definition(&definition);
        // Keyed on the HASH, which is what makes editing an approved server re-prompt.
        let approved = approvals.approvals.iter().any(|entry| {
            entry.project_root == project_root
                && entry.server_name == *name
                && entry.definition_hash == definition_hash
        });
        if ctx.project_trusted
            && (approved || (!ctx.has_ui && ctx.policy == ProjectServerPolicy::Allow))
        {
            continue;
        }

        let reason = if !ctx.project_trusted {
            ProjectServerBlockReason::Untrusted
        } else if !ctx.has_ui {
            ProjectServerBlockReason::ApprovalRequired
        } else {
            let prompt = format!(
                "Allow project MCP server \u{201c}{name}\u{201d}?\nProject config: {}\nEndpoint: {}\n\nThis server can run local commands or make network requests with your user permissions.",
                source.path.display(),
                describe_server(&definition)
            );
            let chosen = match ctx.ui {
                Some(dialog) => dialog.select(&prompt, &PROJECT_SERVER_CHOICES).await,
                // `hasUI` without a surface: nothing was asked, so nothing was allowed.
                None => None,
            };
            if chosen.as_deref() == Some(ALLOW_PROJECT_SERVER) {
                if let Err(error) = approve_project_server(ctx.dirs, ctx.cwd, name, &definition) {
                    // The approval ran, and only its persistence failed. The server is admitted
                    // for THIS session — the user said yes — and will be asked again next time,
                    // which is the conservative direction for a store that cannot be written.
                    tracing::warn!(
                        "MCP: could not record the approval for project server \"{name}\": {error}"
                    );
                }
                continue;
            }
            // A dismissal is a denial: `=== ALLOW_PROJECT_SERVER` is false for `None` too.
            ProjectServerBlockReason::Denied
        };

        // `config.mcpServers[name] = { ...definition, disabled: true }` — the entry STAYS, so every
        // surface can still name the server and say why it is not running.
        let mut blocked = definition;
        blocked.disabled = Some(true);
        let _ = result.config.mcp_servers.insert(name.clone(), blocked);
        let _ = result.blocked_servers.insert(
            name.clone(),
            ProjectServerBlock {
                reason,
                source: source.clone(),
            },
        );
    }

    result
}

/// `excludeProjectServersAtLoadTime(loaded)` (`:235`) — remove project-derived servers outright.
///
/// `4ed656e` (#714). The load-time pass runs before any session exists, so there is no `cwd` to
/// scope approvals by and no UI to ask with; registering a project server's tools there would put
/// them in front of the model before the gate has run at all. They come back at `session_start`,
/// approved or blocked.
#[must_use]
pub fn exclude_project_servers_at_load_time(
    config: &McpConfig,
    project_servers: &IndexMap<String, ProjectServerSource>,
) -> McpConfig {
    let mut config = config.clone();
    for name in project_servers.keys() {
        let _ = config.mcp_servers.shift_remove(name);
    }
    config
}

/// `JSON.stringify(canonicalize(definition))` — the exact bytes
/// [`hash_project_server_definition`] digests.
///
/// Returned separately from the digest for the reason [`crate::dirs::server_identity_pre_image`] is:
/// a hash mismatch says nothing about *which* field disagreed.
#[must_use]
pub fn canonical_json(definition: &ServerEntry) -> String {
    serde_json::to_value(definition)
        .ok()
        .and_then(|value| serde_json::to_string(&canonicalize_json(&value)).ok())
        // Unreachable: `ServerEntry`'s fields are strings, bools, finite numbers and maps of those.
        // Total rather than `unwrap`, and `"null"` cannot collide with a real definition's
        // rendering, which always begins `{`.
        .unwrap_or_else(|| "null".to_string())
}

/// The in-memory view of the store, for a test that wants the records by scope.
#[must_use]
#[doc(hidden)]
pub fn approvals_by_scope(dirs: &McpDirs) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for record in load_approvals(dirs).approvals {
        out.entry(record.project_root)
            .or_default()
            .push(record.server_name);
    }
    out
}

// ===================================================================================================
// Tests — MCP-591
// ===================================================================================================

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use serde_json::Value;

    use super::*;
    use crate::owner::{McpDialog, McpRuntimeOwner, OwnedServices};

    /// A UI that answers one fixed label and records every prompt and option list it was shown.
    #[derive(Default)]
    struct ScriptedUi {
        answer: Mutex<Option<String>>,
        prompts: Mutex<Vec<String>>,
        options: Mutex<Vec<Vec<String>>>,
    }

    impl ScriptedUi {
        fn answering(answer: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer.map(str::to_string)),
                ..Self::default()
            })
        }

        fn prompts(&self) -> Vec<String> {
            self.prompts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn options(&self) -> Vec<Vec<String>> {
            self.options
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl cyrup_ext::HostServices for ScriptedUi {
        fn select(
            &self,
            prompt: &str,
            options: &Value,
            _opts: &cyrup_ext::DialogOptions,
        ) -> Option<String> {
            self.prompts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(prompt.to_string());
            self.options
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(
                    options
                        .as_array()
                        .map(|list| {
                            list.iter()
                                .filter_map(|value| value.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            self.answer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        dirs: McpDirs,
        cwd: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let agent_dir = dir.path().join("agent");
            let cwd = dir.path().join("project");
            std::fs::create_dir_all(&agent_dir).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            Self {
                dirs: McpDirs::new(agent_dir, cwd.clone()),
                cwd,
                _dir: dir,
            }
        }
    }

    fn stdio(command: &str) -> ServerEntry {
        ServerEntry {
            command: Some(command.to_string()),
            ..ServerEntry::default()
        }
    }

    fn config_with(servers: &[(&str, ServerEntry)]) -> McpConfig {
        McpConfig {
            mcp_servers: servers
                .iter()
                .map(|(name, entry)| ((*name).to_string(), entry.clone()))
                .collect(),
            ..McpConfig::default()
        }
    }

    fn project_set(names: &[&str], path: &Path) -> IndexMap<String, ProjectServerSource> {
        names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    ProjectServerSource {
                        path: path.to_path_buf(),
                    },
                )
            })
            .collect()
    }

    fn dialog(ui: &Arc<ScriptedUi>) -> McpDialog {
        McpDialog::new(Arc::clone(ui) as Arc<dyn cyrup_ext::HostServices>)
    }

    /// Each of the three reasons, by the gate that produces it and by its exact string.
    #[tokio::test]
    async fn each_block_reason_has_its_own_exact_string() {
        let fixture = Fixture::new();
        let config = config_with(&[("db", stdio("./serve"))]);
        let servers = project_set(&["db"], &fixture.cwd.join(".mcp.json"));

        // 1 — untrusted. No prompt at all: there is nobody to usefully ask.
        let ui = ScriptedUi::answering(Some(ALLOW_PROJECT_SERVER));
        let surface = dialog(&ui);
        let result = apply_project_server_trust(
            &config,
            &servers,
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: false,
                has_ui: true,
                policy: ProjectServerPolicy::Ask,
                ui: Some(&surface),
            },
        )
        .await;
        assert_eq!(
            result.blocked_servers["db"].reason,
            ProjectServerBlockReason::Untrusted
        );
        assert!(
            ui.prompts().is_empty(),
            "an untrusted project is not offered an approval"
        );
        assert_eq!(
            describe_project_server_block(result.blocked_servers["db"].reason),
            "blocked by project trust — trust the project to review and approve this server"
        );
        assert_eq!(result.config.mcp_servers["db"].disabled, Some(true));

        // 2 — trusted, but no UI and the policy is `ask`.
        let result = apply_project_server_trust(
            &config,
            &servers,
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: true,
                has_ui: false,
                policy: ProjectServerPolicy::Ask,
                ui: None,
            },
        )
        .await;
        assert_eq!(
            describe_project_server_block(result.blocked_servers["db"].reason),
            "blocked: project server approval required — approve it in a trusted interactive session or set user-global settings.projectServers to \"allow\""
        );

        // 3 — trusted, asked, refused.
        let ui = ScriptedUi::answering(Some(DENY_PROJECT_SERVER));
        let surface = dialog(&ui);
        let result = apply_project_server_trust(
            &config,
            &servers,
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: true,
                has_ui: true,
                policy: ProjectServerPolicy::Ask,
                ui: Some(&surface),
            },
        )
        .await;
        assert_eq!(
            describe_project_server_block(result.blocked_servers["db"].reason),
            "blocked: project server approval denied — reload in a trusted interactive session to approve it"
        );
        assert_eq!(ui.prompts().len(), 1);
        assert!(
            ui.prompts()[0].contains("Allow project MCP server \u{201c}db\u{201d}?")
                && ui.prompts()[0].contains("Project config: ")
                && ui.prompts()[0].contains("Endpoint: \"./serve\"")
                && ui.prompts()[0].contains(
                    "This server can run local commands or make network requests with your user permissions."
                ),
            "{}",
            ui.prompts()[0]
        );
    }

    /// `67fcdf9` (#797): the choices are deny-FIRST, so the preselected option refuses. A dismissal
    /// — the dialog closed without a choice — is a denial too.
    #[tokio::test]
    async fn a_stray_enter_and_a_dismissal_both_deny() {
        let fixture = Fixture::new();
        let config = config_with(&[("db", stdio("./serve"))]);
        let servers = project_set(&["db"], &fixture.cwd.join(".mcp.json"));

        // The preselected option is the FIRST, and it is the refusal.
        assert_eq!(PROJECT_SERVER_CHOICES[0], DENY_PROJECT_SERVER);
        assert_eq!(PROJECT_SERVER_CHOICES[1], ALLOW_PROJECT_SERVER);

        for answer in [Some(DENY_PROJECT_SERVER), None] {
            let ui = ScriptedUi::answering(answer);
            let surface = dialog(&ui);
            let result = apply_project_server_trust(
                &config,
                &servers,
                &ProjectTrustContext {
                    cwd: &fixture.cwd,
                    dirs: &fixture.dirs,
                    project_trusted: true,
                    has_ui: true,
                    policy: ProjectServerPolicy::Ask,
                    ui: Some(&surface),
                },
            )
            .await;
            assert_eq!(
                result.blocked_servers["db"].reason,
                ProjectServerBlockReason::Denied,
                "{answer:?}"
            );
            assert_eq!(
                ui.options()[0],
                vec![DENY_PROJECT_SERVER, ALLOW_PROJECT_SERVER],
                "the order the user sees is the order that makes Enter safe"
            );
            // Nothing was written: a refusal is not an approval record.
            assert!(load_approvals(&fixture.dirs).approvals.is_empty());
        }
    }

    /// An approval is stored, re-used without a second prompt, and **invalidated by an edit** —
    /// which is what keying it on `hashProjectServerDefinition` buys.
    #[tokio::test]
    async fn an_approval_persists_and_an_edit_re_prompts() {
        let fixture = Fixture::new();
        let source = fixture.cwd.join(".mcp.json");
        let servers = project_set(&["db"], &source);
        async fn approve(
            fixture: &Fixture,
            servers: &IndexMap<String, ProjectServerSource>,
            config: &McpConfig,
            ui: &Arc<ScriptedUi>,
        ) -> ProjectTrustResult {
            let surface = dialog(ui);
            apply_project_server_trust(
                config,
                servers,
                &ProjectTrustContext {
                    cwd: &fixture.cwd,
                    dirs: &fixture.dirs,
                    project_trusted: true,
                    has_ui: true,
                    policy: ProjectServerPolicy::Ask,
                    ui: Some(&surface),
                },
            )
            .await
        }

        let original = config_with(&[("db", stdio("./serve"))]);
        let ui = ScriptedUi::answering(Some(ALLOW_PROJECT_SERVER));
        let result = approve(&fixture, &servers, &original, &ui).await;
        assert!(result.blocked_servers.is_empty(), "approved, so admitted");
        assert_eq!(result.config.mcp_servers["db"].disabled, None);
        assert_eq!(ui.prompts().len(), 1);

        // The store is readable, scoped, and `0600`.
        let stored = load_approvals(&fixture.dirs);
        assert_eq!(stored.version, APPROVALS_VERSION);
        assert_eq!(stored.approvals.len(), 1);
        assert_eq!(stored.approvals[0].server_name, "db");
        assert_eq!(
            stored.approvals[0].definition_hash,
            hash_project_server_definition(&stdio("./serve"))
        );
        assert!(
            stored.approvals[0].approved_at.ends_with('Z')
                && stored.approvals[0].approved_at.len() == 24,
            "{}",
            stored.approvals[0].approved_at
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(approval_path(&fixture.dirs))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the store records what may run local commands");
        }

        // The same definition again: no prompt.
        let ui = ScriptedUi::answering(Some(DENY_PROJECT_SERVER));
        let result = approve(&fixture, &servers, &original, &ui).await;
        assert!(result.blocked_servers.is_empty());
        assert!(
            ui.prompts().is_empty(),
            "an approved definition is not re-asked"
        );

        // EDITED — a new argument is a new definition, so the approval no longer applies.
        let edited = config_with(&[(
            "db",
            ServerEntry {
                args: Some(vec!["--exfiltrate".to_string()]),
                ..stdio("./serve")
            },
        )]);
        assert_ne!(
            hash_project_server_definition(&stdio("./serve")),
            hash_project_server_definition(&edited.mcp_servers["db"])
        );
        let ui = ScriptedUi::answering(Some(DENY_PROJECT_SERVER));
        let result = approve(&fixture, &servers, &edited, &ui).await;
        assert_eq!(ui.prompts().len(), 1, "editing an approved server re-asks");
        assert_eq!(
            result.blocked_servers["db"].reason,
            ProjectServerBlockReason::Denied
        );

        // And re-approving REPLACES the record rather than appending a second one.
        let ui = ScriptedUi::answering(Some(ALLOW_PROJECT_SERVER));
        let _ = approve(&fixture, &servers, &edited, &ui).await;
        assert_eq!(load_approvals(&fixture.dirs).approvals.len(), 1);
    }

    /// `settings.projectServers: "allow"` waives the prompt for a **non-interactive** session only.
    #[tokio::test]
    async fn the_allow_policy_waives_only_a_non_interactive_prompt() {
        let fixture = Fixture::new();
        let config = config_with(&[("db", stdio("./serve"))]);
        let servers = project_set(&["db"], &fixture.cwd.join(".mcp.json"));

        // Non-interactive + trusted + allow ⇒ admitted with nothing asked and nothing stored.
        let result = apply_project_server_trust(
            &config,
            &servers,
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: true,
                has_ui: false,
                policy: ProjectServerPolicy::Allow,
                ui: None,
            },
        )
        .await;
        assert!(result.blocked_servers.is_empty());
        assert!(load_approvals(&fixture.dirs).approvals.is_empty());

        // INTERACTIVE + allow still asks: a human is there to be asked.
        let ui = ScriptedUi::answering(Some(DENY_PROJECT_SERVER));
        let surface = dialog(&ui);
        let result = apply_project_server_trust(
            &config,
            &servers,
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: true,
                has_ui: true,
                policy: ProjectServerPolicy::Allow,
                ui: Some(&surface),
            },
        )
        .await;
        assert_eq!(ui.prompts().len(), 1);
        assert_eq!(
            result.blocked_servers["db"].reason,
            ProjectServerBlockReason::Denied
        );

        // `allow` does NOT override project trust.
        let result = apply_project_server_trust(
            &config,
            &servers,
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: false,
                has_ui: false,
                policy: ProjectServerPolicy::Allow,
                ui: None,
            },
        )
        .await;
        assert_eq!(
            result.blocked_servers["db"].reason,
            ProjectServerBlockReason::Untrusted
        );
    }

    /// `7dc3d28` (#688) — a project server that is already **disabled** is not offered for
    /// approval: it is not going to start either way, and asking would train the user to approve
    /// reflexively. A name with no definition left in the merged table is skipped too.
    #[tokio::test]
    async fn a_disabled_or_outranked_project_server_is_never_prompted() {
        let fixture = Fixture::new();
        let config = config_with(&[(
            "db",
            ServerEntry {
                disabled: Some(true),
                ..stdio("./serve")
            },
        )]);
        let ui = ScriptedUi::answering(Some(ALLOW_PROJECT_SERVER));
        let surface = dialog(&ui);
        let result = apply_project_server_trust(
            &config,
            &project_set(&["db", "gone"], &fixture.cwd.join(".mcp.json")),
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: true,
                has_ui: true,
                policy: ProjectServerPolicy::Ask,
                ui: Some(&surface),
            },
        )
        .await;
        assert!(ui.prompts().is_empty());
        assert!(result.blocked_servers.is_empty());
    }

    /// A corrupt or wrong-version store fails to EMPTY — so every project server asks again — and
    /// never to "everything approved".
    #[test]
    fn an_unusable_approval_store_makes_everything_ask_again() {
        let fixture = Fixture::new();
        let path = approval_path(&fixture.dirs);
        for text in [
            "{{{",
            "[]",
            r#"{"version": 2, "approvals": []}"#,
            r#"{"version": 1}"#,
            r#"{"version": 1, "approvals": "nope"}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(load_approvals(&fixture.dirs).approvals.is_empty(), "{text}");
        }
        // A single malformed RECORD is dropped and its neighbours survive.
        std::fs::write(
            &path,
            r#"{"version":1,"approvals":[{"projectRoot":"/p","serverName":"a","definitionHash":"h","approvedAt":"t"},{"projectRoot":5}]}"#,
        )
        .unwrap();
        let store = load_approvals(&fixture.dirs);
        assert_eq!(store.approvals.len(), 1);
        assert_eq!(store.approvals[0].server_name, "a");
    }

    /// `projectApprovalScope` — two worktrees of one repository share approvals, and an unverified
    /// `.git` file cannot borrow a checkout's.
    #[test]
    fn worktrees_share_approvals_and_a_planted_pointer_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();

        // A regular checkout at `<root>/repo` with a linked worktree at `<root>/wt`.
        let repo = root.join("repo");
        let git_dir = repo.join(".git");
        let admin = git_dir.join("worktrees").join("wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(git_dir.join("config"), "[core]\n\tbare = false\n").unwrap();
        let worktree = root.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .unwrap();

        // The checkout's own scope is its path, and the worktree resolves to the SAME repository
        // scope, so an approval made in one applies in the other.
        assert_eq!(
            project_approval_scope(&repo),
            repo.to_string_lossy().into_owned()
        );
        assert_eq!(
            project_approval_scope(&worktree),
            project_approval_scope(&repo)
        );

        // A `.git` file whose admin entry does NOT link back cannot claim that scope.
        let planted = root.join("planted");
        std::fs::create_dir_all(&planted).unwrap();
        std::fs::write(
            planted.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        assert_eq!(
            project_approval_scope(&planted),
            planted.to_string_lossy().into_owned(),
            "the back-link check refuses it, so it gets its own scope and borrows nothing"
        );

        // A BARE repository's worktree gets a `git-dir:` key, which no canonical path equals.
        let bare = root.join("bare.git");
        let bare_admin = bare.join("worktrees").join("w");
        std::fs::create_dir_all(&bare_admin).unwrap();
        std::fs::write(bare.join("config"), "[core]\n\tbare = true\n").unwrap();
        let bare_worktree = root.join("bw");
        std::fs::create_dir_all(&bare_worktree).unwrap();
        std::fs::write(
            bare_worktree.join(".git"),
            format!("gitdir: {}\n", bare_admin.display()),
        )
        .unwrap();
        std::fs::write(
            bare_admin.join("gitdir"),
            format!("{}\n", bare_worktree.join(".git").display()),
        )
        .unwrap();
        let scope = project_approval_scope(&bare_worktree);
        assert!(scope.starts_with("git-dir:"), "{scope}");
        assert_ne!(scope, bare_worktree.to_string_lossy().into_owned());

        // `bare = yes` is bare too — git's own truthy spellings, not just `true`.
        assert!(config_says_bare("[core]\n\tbare = yes\n"));
        assert!(config_says_bare("bare=1"));
        assert!(config_says_bare("  BARE = On "));
        assert!(!config_says_bare("[core]\n\tbare = false\n"));
        assert!(!config_says_bare("barely = true"));
    }

    /// A `.git` **symlink** to a real worktree's pointer is refused — `lstatSync(...).isFile()`.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_git_pointer_is_not_a_linked_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        let admin = repo.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(repo.join(".git").join("config"), "bare = false\n").unwrap();
        let worktree = root.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .unwrap();

        let copycat = root.join("copycat");
        std::fs::create_dir_all(&copycat).unwrap();
        std::os::unix::fs::symlink(worktree.join(".git"), copycat.join(".git")).unwrap();
        assert_eq!(
            project_approval_scope(&copycat),
            copycat.to_string_lossy().into_owned(),
            "a symlink to someone else's pointer is not a worktree of their repository"
        );
    }

    /// The hash is sensitive to every field and insensitive to nothing but field ORDER.
    #[test]
    fn the_definition_hash_is_order_insensitive_and_field_sensitive() {
        let a: ServerEntry =
            serde_json::from_str(r#"{"command":"x","args":["1"],"env":{"B":"2","A":"1"}}"#)
                .unwrap();
        let b: ServerEntry =
            serde_json::from_str(r#"{"env":{"A":"1","B":"2"},"args":["1"],"command":"x"}"#)
                .unwrap();
        assert_eq!(
            hash_project_server_definition(&a),
            hash_project_server_definition(&b),
            "canonicalize sorts keys at every depth"
        );
        // Arrays keep their order, so reordering `args` IS a different definition.
        let reordered: ServerEntry =
            serde_json::from_str(r#"{"command":"x","args":["1","2"]}"#).unwrap();
        let swapped: ServerEntry =
            serde_json::from_str(r#"{"command":"x","args":["2","1"]}"#).unwrap();
        assert_ne!(
            hash_project_server_definition(&reordered),
            hash_project_server_definition(&swapped)
        );
        // An ABSENT field is not an empty one.
        assert_ne!(
            hash_project_server_definition(&stdio("x")),
            hash_project_server_definition(&ServerEntry {
                args: Some(Vec::new()),
                ..stdio("x")
            })
        );
        assert!(
            canonical_json(&a).starts_with(r#"{"args":"#),
            "{}",
            canonical_json(&a)
        );
    }

    /// `describeServer` — `command` wins, each element is `JSON.stringify`'d so a quoted argument
    /// cannot forge two.
    #[test]
    fn describe_server_quotes_every_element() {
        assert_eq!(
            describe_server(&ServerEntry {
                args: Some(vec!["a b".to_string(), "\"c\"".to_string()]),
                url: Some("https://ignored.example".to_string()),
                ..stdio("./serve")
            }),
            r#""./serve" "a b" "\"c\"""#
        );
        assert_eq!(
            describe_server(&ServerEntry {
                url: Some("https://a.example/mcp".to_string()),
                ..ServerEntry::default()
            }),
            "https://a.example/mcp"
        );
        assert_eq!(
            describe_server(&ServerEntry::default()),
            "(no command or endpoint)"
        );
    }

    /// `disabledServerReason` — the block reason when there is one, the `/mcp enable` sentence
    /// otherwise.
    #[test]
    fn the_disabled_reason_switches_on_the_block_map() {
        let mut blocked = IndexMap::new();
        let _ = blocked.insert(
            "db".to_string(),
            ProjectServerBlock {
                reason: ProjectServerBlockReason::Untrusted,
                source: ProjectServerSource {
                    path: PathBuf::from("/p/.mcp.json"),
                },
            },
        );
        assert_eq!(
            disabled_server_reason(Some(&blocked), "db"),
            "blocked by project trust — trust the project to review and approve this server"
        );
        assert_eq!(
            disabled_server_reason(Some(&blocked), "other"),
            "disabled. Run /mcp enable other and /reload to enable it."
        );
        assert_eq!(
            disabled_server_reason(None, "db"),
            "disabled. Run /mcp enable db and /reload to enable it."
        );
    }

    /// `excludeProjectServersAtLoadTime` removes exactly the project set and nothing else.
    #[test]
    fn the_load_time_pass_removes_project_servers_only() {
        let config = config_with(&[("mine", stdio("a")), ("theirs", stdio("b"))]);
        let reduced = exclude_project_servers_at_load_time(
            &config,
            &project_set(&["theirs"], Path::new("/p/.mcp.json")),
        );
        assert_eq!(reduced.mcp_servers.len(), 1);
        assert!(reduced.mcp_servers.contains_key("mine"));
        assert!(has_project_server_definitions(&project_set(
            &["theirs"],
            Path::new("/p")
        )));
        assert!(!has_project_server_definitions(&IndexMap::new()));
    }

    /// The owner-fenced surface is inert after a stop, so a dialog on a dead generation refuses
    /// rather than approving.
    #[tokio::test]
    async fn a_dead_generation_cannot_approve() {
        let fixture = Fixture::new();
        let owner = Arc::new(McpRuntimeOwner::new());
        let ui = ScriptedUi::answering(Some(ALLOW_PROJECT_SERVER));
        let fenced = Arc::new(OwnedServices::new(
            Arc::clone(&ui) as Arc<dyn cyrup_ext::HostServices>,
            Arc::clone(&owner),
        ));
        let _ = owner.begin_stop(Some("test")).await;
        let surface = McpDialog::fenced(&fenced);

        let result = apply_project_server_trust(
            &config_with(&[("db", stdio("./serve"))]),
            &project_set(&["db"], &fixture.cwd.join(".mcp.json")),
            &ProjectTrustContext {
                cwd: &fixture.cwd,
                dirs: &fixture.dirs,
                project_trusted: true,
                has_ui: true,
                policy: ProjectServerPolicy::Ask,
                ui: Some(&surface),
            },
        )
        .await;
        assert_eq!(
            result.blocked_servers["db"].reason,
            ProjectServerBlockReason::Denied
        );
        assert!(
            ui.prompts().is_empty(),
            "the fence answered `None` without reaching the backend"
        );
    }
}
