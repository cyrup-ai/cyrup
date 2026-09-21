//! Discovery: am I inside a herdr pane, and where is the socket?
//!
//! Two questions with deliberately different answers.
//!
//! [`HerdrPane::discover`] answers the first and is the **inert gate**: outside a herdr pane it
//! returns `None` and the caller starts nothing — no client, no connect, no task, no thread. It
//! does not fall through to a default socket path, because a cyrup session in a plain terminal
//! must not open a connection to whatever herdr session happens to be running under the same user.
//!
//! [`resolve_socket_path`] answers the second and is a **deliberate escape hatch**: a caller that
//! has decided it wants to reach a running herdr server — the inspector verbs, say, which are a
//! user asking for herdr by name — replicates herdr's own ladder to find it. That is
//! `[CYRUP-EXCEEDS-UPSTREAM]` over pi, whose `SocketRpcClient`
//! (`src/runs/shared/herdr-connection.ts:70-107` @v0.68.0) only ever receives a path handed to it.
//! **Premise — with only `HERDR_SOCKET_PATH`, a cyrup session started outside a herdr pane but
//! alongside a running herdr server could not find it, and that is the maintainer's normal state.**

use std::path::{Path, PathBuf};

use crate::error::{Result, Unavailable};

/// Where environment variables come from.
///
/// A trait rather than `std::env::var` calls, for one reason that is not style: `cargo test` runs a
/// crate's unit tests as parallel threads in **one** process, so a test that mutates the process
/// environment is racing every other test in the binary. Every test here therefore hands the
/// resolution ladder a map. In production the implementation is [`ProcessEnv`], which is
/// `std::env::var` — the same source herdr itself reads (`tmp/herdr/src/config/io.rs:31`,
/// `tmp/herdr/src/session.rs:97`).
pub trait EnvSource {
    /// The value of `key`, or `None` if unset (or not valid Unicode).
    fn var(&self, key: &str) -> Option<String>;
}

/// The real process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

impl EnvSource for std::collections::HashMap<String, String> {
    fn var(&self, key: &str) -> Option<String> {
        self.get(key).cloned()
    }
}

impl EnvSource for std::collections::BTreeMap<String, String> {
    fn var(&self, key: &str) -> Option<String> {
        self.get(key).cloned()
    }
}

impl<T: EnvSource + ?Sized> EnvSource for &T {
    fn var(&self, key: &str) -> Option<String> {
        (**self).var(key)
    }
}

/// `HERDR_ENV` (`tmp/herdr/src/main.rs:3`).
pub const HERDR_ENV: &str = "HERDR_ENV";
/// The value `HERDR_ENV` carries inside a herdr-launched process
/// (`tmp/herdr/src/main.rs:4`, applied at `tmp/herdr/src/pane.rs:156`).
pub const HERDR_ENV_VALUE: &str = "1";
/// `HERDR_SOCKET_PATH` (`tmp/herdr/src/api/mod.rs:20`, injected at
/// `tmp/herdr/src/integration/env.rs:29`).
pub const HERDR_SOCKET_PATH: &str = "HERDR_SOCKET_PATH";
/// `HERDR_PANE_ID` (`tmp/herdr/src/integration/env.rs:8`), e.g. `w1:p1`.
pub const HERDR_PANE_ID: &str = "HERDR_PANE_ID";
/// `HERDR_TAB_ID` (`tmp/herdr/src/integration/env.rs:9`), e.g. `w1:t1`.
pub const HERDR_TAB_ID: &str = "HERDR_TAB_ID";
/// `HERDR_WORKSPACE_ID` (`tmp/herdr/src/integration/env.rs:10`), e.g. `w1`.
///
/// The seed listed four injected variables; herdr injects five
/// (`socket-api.mdx:302-305`, `tmp/herdr/src/pane.rs:166-168`).
pub const HERDR_WORKSPACE_ID: &str = "HERDR_WORKSPACE_ID";
/// `HERDR_SESSION` (`tmp/herdr/src/session.rs:10`) — rung 3 of the socket ladder.
pub const HERDR_SESSION: &str = "HERDR_SESSION";

/// herdr's config directory name.
///
/// **Hard-coded, and that is the correct behaviour rather than a shortcut.** herdr's own
/// `app_dir_name` (`tmp/herdr/src/config/io.rs:22-28`) answers `"herdr-dev"` under
/// `cfg!(debug_assertions)` and `"herdr"` otherwise — but that reads *herdr's* build profile, and
/// this code is compiled into *cyrup*. Deriving it from `cfg!(debug_assertions)` here would make a
/// debug build of cyrup look for `~/.config/herdr-dev` while the user's released herdr listens on
/// `~/.config/herdr`, i.e. a client that works in release and silently finds nothing in debug. A
/// developer running a debug herdr sets `HERDR_SOCKET_PATH` (herdr injects it) or
/// `XDG_CONFIG_HOME`, both of which are rungs above this one.
const HERDR_APP_DIR: &str = "herdr";

/// herdr's socket file name (`tmp/herdr/src/session.rs:169-171`).
const HERDR_SOCKET_FILE: &str = "herdr.sock";

/// `MAX_SESSION_NAME_LEN` — herdr rejects a longer `HERDR_SESSION`
/// (`tmp/herdr/src/session.rs:456-460`).
const MAX_SESSION_NAME_LEN: usize = 64;

/// The herdr pane this process is running inside.
///
/// Obtained only from [`Self::discover`]; there is no other constructor, so a value of this type is
/// proof that the gate passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrPane {
    socket_path: PathBuf,
    pane_id: String,
    tab_id: Option<String>,
    workspace_id: Option<String>,
}

impl HerdrPane {
    /// The gate. `None` means **inert**: the caller creates no client, opens no connection, spawns
    /// no task.
    ///
    /// Both halves of the condition are load-bearing:
    ///
    /// - `HERDR_ENV == "1"` — herdr sets it on every pane process it launches
    ///   (`tmp/herdr/src/pane.rs:156`).
    /// - `HERDR_PANE_ID` non-empty — herdr **removes** it for a `PaneLaunchIdentity::OmitPane`
    ///   launch (`tmp/herdr/src/pane.rs:170-172`) while leaving `HERDR_ENV=1` and
    ///   `HERDR_SOCKET_PATH` in place. So a process can sit inside herdr, see a live socket, and
    ///   still own no pane. Reporting agent state against a pane id it does not have is exactly the
    ///   fabricated success this crate refuses.
    ///
    /// pi's inspector plugin gates on the same two (`src/integrations/herdr-status.ts:130-131`
    /// @v0.68.0).
    ///
    /// Only once the gate passes is the socket path resolved, by [`resolve_socket_path`].
    #[must_use]
    pub fn discover(env: &impl EnvSource) -> Option<Self> {
        if env.var(HERDR_ENV).as_deref() != Some(HERDR_ENV_VALUE) {
            tracing::trace!("not a herdr pane: HERDR_ENV is not \"1\"");
            return None;
        }
        let pane_id = env.var(HERDR_PANE_ID).filter(|id| !id.is_empty())?;
        Some(Self {
            socket_path: resolve_socket_path(env),
            pane_id,
            tab_id: env.var(HERDR_TAB_ID).filter(|id| !id.is_empty()),
            workspace_id: env.var(HERDR_WORKSPACE_ID).filter(|id| !id.is_empty()),
        })
    }

    /// [`Self::discover`], for a caller that was **asked** to do a herdr thing and owes the user a
    /// reason when it cannot.
    ///
    /// The distinction is the whole of §9's first row. A session start is ambient: it calls
    /// [`Self::discover`], gets `None` outside a pane, and does nothing at all — no client, no
    /// connect, no task, no log above `trace`. A user typing an inspector verb is not ambient: it
    /// calls this, and an answer of [`Unavailable::NotInHerdrPane`] is the sentence it shows.
    ///
    /// # Errors
    /// [`Unavailable::NotInHerdrPane`] when the gate does not pass. There is no other failure —
    /// resolving the socket path never fails, and whether anything is listening on it is not known
    /// until the first call.
    pub fn require(env: &impl EnvSource) -> Result<Self> {
        Self::discover(env).ok_or(Unavailable::NotInHerdrPane.into())
    }

    /// The resolved socket path for this pane's server.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// This process's pane, e.g. `w1:p1`. Never empty — the gate rejects an empty value.
    #[must_use]
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }

    /// The pane's tab, e.g. `w1:t1`, when herdr injected one.
    #[must_use]
    pub fn tab_id(&self) -> Option<&str> {
        self.tab_id.as_deref()
    }

    /// The pane's workspace, e.g. `w1`, when herdr injected one.
    #[must_use]
    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }
}

/// herdr's own socket-path ladder, replicated.
///
/// Mirrors `active_api_socket_path` (`tmp/herdr/src/session.rs:173-181`) →
/// `api_socket_path_for` (`:169-171`) → `data_dir_for` (`:161-167`) →
/// `config_dir` (`tmp/herdr/src/config/io.rs:30-35`); documented at `socket-api.mdx:686-692`.
///
/// 1. **skipped** — an explicit `--session <name>` on herdr's own CLI, which sets a process-global
///    flag: `apply_explicit_name` stores it (`EXPLICIT_SESSION_REQUESTED`,
///    `tmp/herdr/src/session.rs:475-484`, the `store(true)` on `:482`) and `active_api_socket_path`
///    reads it (`:173-174`). There is no environment equivalent — only herdr's own `--session`
///    flag reaches that setter (`:76-91`) — so a foreign client cannot observe it, and putting
///    `HERDR_SOCKET_PATH` first is correct for an env-only client.
/// 2. `HERDR_SOCKET_PATH`, verbatim.
/// 3. `HERDR_SESSION=<name>` → `<config_dir>/sessions/<name>/herdr.sock`, where `<name>` passes
///    herdr's filter: not `"default"` and `validate_name`-clean (`tmp/herdr/src/session.rs:96-101`,
///    `:452-472`). A rejected name falls through to rung 4 — it does **not** become a literal
///    directory, which is what dropping the filter would silently do.
/// 4. `<config_dir>/herdr.sock`.
///
/// `<config_dir>` is `$XDG_CONFIG_HOME/herdr` when set, else the platform directory — see
/// [`config_dir`].
#[must_use]
pub fn resolve_socket_path(env: &impl EnvSource) -> PathBuf {
    if let Some(path) = env.var(HERDR_SOCKET_PATH).filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let dir = config_dir(env);
    match active_session_name(env) {
        Some(name) => dir.join("sessions").join(name).join(HERDR_SOCKET_FILE),
        None => dir.join(HERDR_SOCKET_FILE),
    }
}

/// `active_name` (`tmp/herdr/src/session.rs:96-101`): `HERDR_SESSION`, filtered.
fn active_session_name(env: &impl EnvSource) -> Option<String> {
    env.var(HERDR_SESSION)
        .filter(|name| name != "default")
        .filter(|name| validate_session_name(name).is_ok())
}

/// `validate_name` (`tmp/herdr/src/session.rs:452-472`), reproduced including its messages.
///
/// Reproduced rather than reduced to a boolean because the name is a path component: a name herdr
/// rejects must not be joined onto `<config_dir>/sessions/`, or a `HERDR_SESSION=../../etc` would
/// resolve a socket path outside herdr's own config tree.
fn validate_session_name(name: &str) -> core::result::Result<(), String> {
    if name.is_empty() {
        return Err("session name cannot be empty".to_string());
    }
    if name.len() > MAX_SESSION_NAME_LEN {
        return Err(format!(
            "session name cannot be longer than {MAX_SESSION_NAME_LEN} bytes"
        ));
    }
    if name == "." || name == ".." {
        return Err("session name cannot be . or ..".to_string());
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(
            "session name may only contain ASCII letters, numbers, '.', '_' and '-'".to_string(),
        );
    }
    Ok(())
}

/// herdr's `config_dir` (`tmp/herdr/src/config/io.rs:30-35`) and its platform fallbacks
/// (`:45-58` on Windows, `:62-68` elsewhere).
///
/// `$XDG_CONFIG_HOME/herdr` wins everywhere — herdr checks it before any platform directory,
/// including on Windows. Otherwise:
///
/// - Windows: `%APPDATA%\herdr`, then `%USERPROFILE%\AppData\Roaming\herdr`, then
///   `$HOME/.config/herdr`, then the temp dir.
/// - Elsewhere: `$HOME/.config/herdr`, then the temp dir.
///
/// The platform arms are selected by `cfg!(windows)` on **this** build rather than `#[cfg]`, so
/// both ladders are compiled and type-checked on every target and the Windows arm cannot rot into
/// unbuildable text — the failure `crates/cyrup-intercom/src/broker/listener.rs:16-25` records
/// having already happened once in this workspace.
#[must_use]
pub fn config_dir(env: &impl EnvSource) -> PathBuf {
    if let Some(dir) = env.var("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join(HERDR_APP_DIR);
    }
    platform_config_dir(env)
}

fn platform_config_dir(env: &impl EnvSource) -> PathBuf {
    if cfg!(windows) {
        if let Some(dir) = env.var("APPDATA").filter(|dir| !dir.is_empty()) {
            return PathBuf::from(dir).join(HERDR_APP_DIR);
        }
        if let Some(profile) = env.var("USERPROFILE").filter(|dir| !dir.is_empty()) {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join(HERDR_APP_DIR);
        }
    }
    if let Some(home) = env.var("HOME").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(home).join(".config").join(HERDR_APP_DIR);
    }
    std::env::temp_dir().join(HERDR_APP_DIR)
}
