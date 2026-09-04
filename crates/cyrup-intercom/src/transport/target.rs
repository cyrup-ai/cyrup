//! Broker **transport-target** resolution — a port of `pi-intercom/broker/paths.ts:7-25,44-59,
//! 61-63,65-74,76-116` (pi-intercom v0.7.0, the ported baseline).
//!
//! pi-intercom does not always talk to a Unix domain socket. `getBrokerSocketPath`
//! (`paths.ts:65-74`) selects a **Windows named pipe** (`\\.\pipe\pi-intercom-<sanitized agent
//! dir>`) on `win32`, and `shouldUseWindowsTcpTransport` (`paths.ts:44-59`) additionally allows an
//! **opt-in TCP loopback** transport on Windows only, gated on `PI_INTERCOM_TRANSPORT=tcp` (or the
//! legacy `PI_INTERCOM_TCP=1|true`). Under TCP the broker binds `127.0.0.1:0` and publishes the
//! chosen port plus a per-broker-run credential (`stateId`) into `<intercomDir>/broker.port.json`
//! (`broker.ts:64-85`); every client then reads that file (`getBrokerConnectTarget`,
//! `paths.ts:76-105`) and must echo the `stateId` on `health`/`register` or the broker throws
//! `Invalid intercom TCP endpoint credentials` (`broker.ts:250-271`).
//!
//! Platform is an explicit **parameter** here, exactly as it is upstream (`paths.ts:45,66,77,108`
//! all take `platform: NodeJS.Platform = process.platform`). That is what makes the Windows/TCP
//! selection rules testable on any host, and this module's tests mirror `broker/paths.test.ts`
//! case-for-case.
//!
//! This module is also the single home for the broker's **runtime-file paths** under
//! `<intercomDir>`: [`broker_port_file_path`] (`paths.ts:61-63`), [`unix_socket_path`]
//! (`getBrokerSocketPath`'s POSIX branch, `paths.ts:65-74`), [`broker_pid_path`] (`broker.ts:22`)
//! and [`broker_spawn_lock_path`] (`spawn.ts:24`). They used to be split across `crate::paths` (the
//! POSIX socket/pid/lock trio) and this module (the port file), so one upstream question — "where
//! does the broker live" — had two answers at two tree levels and the crate-root one was the
//! shorter, more obvious import even though it hard-codes the POSIX arm. `crate::paths` keeps only
//! the cyrup-home/agent-dir resolution and the runtime-dir/mode helpers, which are not
//! transport-specific, and re-exports these three for the call sites that still spell them
//! `paths::…`.
//!
//! Naming: pi's `PI_INTERCOM_TRANSPORT`/`PI_INTERCOM_TCP` become `CYRUP_INTERCOM_TRANSPORT`/
//! `CYRUP_INTERCOM_TCP`, matching this crate's existing rename of every `PI_INTERCOM_*` var
//! (`identity.rs:20-24`). The pipe-name prefix and the `broker.port.json` file name are kept
//! **byte-identical** to pi's, for the same reason `PROTOCOL_NAME` is
//! (`protocol.rs:10-13`): they are part of the broker-discovery contract, so a cyrup client and a pi
//! broker sharing one agent dir still find each other.

use std::path::{Path, PathBuf};

use crate::error::{IntercomError, Result};

/// `INTERCOM_TCP_HOST = "127.0.0.1"` (`paths.ts:7`). The opt-in TCP transport is loopback-only; a
/// `broker.port.json` naming any other host is rejected (`paths.ts:91`).
pub const INTERCOM_TCP_HOST: &str = "127.0.0.1";

/// `CYRUP_INTERCOM_TRANSPORT` (`paths.ts:52`, pi `PI_INTERCOM_TRANSPORT`): `tcp` opts into the
/// loopback TCP transport on Windows.
pub const ENV_INTERCOM_TRANSPORT: &str = "CYRUP_INTERCOM_TRANSPORT";
/// `CYRUP_INTERCOM_TCP` (`paths.ts:57`, pi `PI_INTERCOM_TCP`): the legacy `1`/`true` opt-in, still
/// honored (`paths.ts:57-58`).
pub const ENV_INTERCOM_TCP: &str = "CYRUP_INTERCOM_TCP";

/// The named-pipe prefix (`paths.ts:70`), kept byte-identical to pi's.
const PIPE_PREFIX: &str = r"\\.\pipe\pi-intercom-";

/// The host platform, as an explicit parameter — pi's `NodeJS.Platform` narrowed to the only
/// distinction any of `paths.ts:44-135` actually makes (`platform === "win32"` vs everything else).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// pi's `"win32"`.
    Windows,
    /// Every other `NodeJS.Platform` value (`"linux"`, `"darwin"`, ...).
    Unix,
}

impl Platform {
    /// The platform this binary was compiled for (pi's `process.platform` default parameter).
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    /// Whether this is pi's `"win32"`.
    #[must_use]
    pub const fn is_windows(self) -> bool {
        matches!(self, Self::Windows)
    }
}

/// `BrokerTcpEndpoint` (`paths.ts:11-16`): the loopback TCP endpoint published by the broker into
/// `broker.port.json` and read back by every client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerTcpEndpoint {
    /// Always [`INTERCOM_TCP_HOST`] on a parsed endpoint (`paths.ts:91`).
    pub host: String,
    /// The listening port; `0` on a *listen* target (bind-any, `paths.ts:112`), a concrete port on a
    /// *connect* target (`paths.ts:92-95` rejects `0`).
    pub port: u16,
    /// The per-broker-run credential every client must echo on `health`/`register`
    /// (`broker.ts:76,250-271`). `None` only on the listen target, which the broker itself fills in
    /// from its own `BROKER_STATE_ID` when it publishes the file (`broker.ts:73-79`).
    pub state_id: Option<String>,
}

impl BrokerTcpEndpoint {
    /// The exact `broker.port.json` body the broker writes:
    /// `` `${JSON.stringify(endpoint)}\n` `` (`broker.ts:79`).
    #[must_use]
    pub fn to_port_file_body(&self) -> String {
        let mut value = serde_json::json!({
            "transport": "tcp",
            "host": self.host,
            "port": self.port,
        });
        if let Some(state_id) = &self.state_id
            && let Some(obj) = value.as_object_mut()
        {
            obj.insert(
                "stateId".to_string(),
                serde_json::Value::String(state_id.clone()),
            );
        }
        format!("{value}\n")
    }
}

/// `BrokerConnectTarget = string | BrokerTcpEndpoint` (`paths.ts:18`).
///
/// The `string` arm is a Unix socket path on non-Windows and a named-pipe name on Windows
/// (`paths.ts:65-74`); both are carried as a [`PathBuf`] here because that is the type this crate's
/// existing socket-path API already speaks ([`unix_socket_path`]), and `\\.\pipe\...` is a
/// well-formed Windows path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerConnectTarget {
    /// A Unix domain socket path (non-Windows) or a Windows named-pipe name.
    Socket(PathBuf),
    /// The opt-in loopback TCP endpoint.
    Tcp(BrokerTcpEndpoint),
}

impl BrokerConnectTarget {
    /// The `stateId` to echo on `health`/`register`, if this target is a TCP endpoint
    /// (`client.ts:284`, `spawn.ts:274,290`). Always `None` for a socket/pipe target — pi spreads
    /// `...(typeof target === "string" ? {} : { stateId: target.stateId })`, i.e. the field is
    /// *omitted*, not sent as null.
    #[must_use]
    pub fn state_id(&self) -> Option<&str> {
        match self {
            Self::Socket(_) => None,
            Self::Tcp(endpoint) => endpoint.state_id.as_deref(),
        }
    }

    /// Whether this target is the `string` arm (`typeof target === "string"`), i.e. a socket path or
    /// a named pipe rather than TCP.
    #[must_use]
    pub const fn is_socket(&self) -> bool {
        matches!(self, Self::Socket(_))
    }

    /// Whether a session registering over this target is `trustedLocal` (`broker.ts:196`:
    /// `typeof LISTEN_TARGET === "string" && process.platform !== "win32"`) — a peer is trusted only
    /// on a real Unix domain socket, never on a named pipe and never over TCP.
    #[must_use]
    pub const fn is_trusted_local(&self, platform: Platform) -> bool {
        self.is_socket() && !platform.is_windows()
    }
}

/// `sanitizePipeSegment` (`paths.ts:20-25`): collapse every run of non-alphanumerics to `-`, trim
/// leading/trailing `-`, lowercase, and fall back to `"default"` when nothing survives.
#[must_use]
fn sanitize_pipe_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(ch);
        } else {
            // A run of non-alphanumerics becomes a single `-`; a trailing run is dropped by the
            // second `.replace(/^-+|-+$/g, "")`, so only emit it once a kept char follows.
            pending_dash = !out.is_empty();
        }
    }
    let out = out.to_lowercase();
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

/// `shouldUseWindowsTcpTransport` (`paths.ts:44-59`): TCP is Windows-only **and** opt-in —
/// `CYRUP_INTERCOM_TRANSPORT=tcp` (case/space-insensitive), or the legacy
/// `CYRUP_INTERCOM_TCP=1|true`. On any non-Windows platform this is unconditionally `false`, even
/// with the env vars set (`paths.ts:48-50`).
#[must_use]
pub fn should_use_windows_tcp_transport(
    platform: Platform,
    env: impl Fn(&str) -> Option<String>,
) -> bool {
    if !platform.is_windows() {
        return false;
    }
    if let Some(transport) = env(ENV_INTERCOM_TRANSPORT)
        && transport.trim().to_lowercase() == "tcp"
    {
        return true;
    }
    let legacy = env(ENV_INTERCOM_TCP).unwrap_or_default();
    let legacy = legacy.trim().to_lowercase();
    legacy == "1" || legacy == "true"
}

/// `getBrokerPortFilePath` (`paths.ts:61-63`): `<intercomDir>/broker.port.json`.
#[must_use]
pub fn broker_port_file_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.port.json")
}

/// `<intercomDir>/broker.sock` — `getBrokerSocketPath`'s **Unix branch only** (`paths.ts:65-74`).
///
/// Named `unix_socket_path`, not `broker_socket_path`, precisely because it is one arm of a
/// two-arm upstream function: on Windows `getBrokerSocketPath` returns the named pipe
/// `\\.\pipe\pi-intercom-<sanitized agent dir>` instead and this path is never bound or dialed.
/// The general, platform-choosing spelling is [`broker_socket_path_for`]; the general
/// *transport*-choosing spellings (which additionally cover the opt-in loopback TCP arm) are
/// [`broker_connect_target`] for clients and [`broker_listen_target`] for the broker. Call this one
/// only where the POSIX arm is what is genuinely meant.
#[must_use]
pub fn unix_socket_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.sock")
}

/// `<intercomDir>/broker.pid` (`broker.ts:22`).
#[must_use]
pub fn broker_pid_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.pid")
}

/// `<intercomDir>/broker.spawn.lock` (`spawn.ts:24`).
#[must_use]
pub fn broker_spawn_lock_path(intercom_dir: &Path) -> PathBuf {
    intercom_dir.join("broker.spawn.lock")
}

/// `getBrokerSocketPath` (`paths.ts:65-74`): the named pipe `\\.\pipe\pi-intercom-<sanitized agent
/// dir>` on Windows, else `<agentDir>/intercom/broker.sock`.
#[must_use]
pub fn broker_socket_path_for(platform: Platform, agent_dir: &Path) -> PathBuf {
    if platform.is_windows() {
        let segment = sanitize_pipe_segment(&agent_dir.to_string_lossy());
        return PathBuf::from(format!("{PIPE_PREFIX}{segment}"));
    }
    unix_socket_path(&crate::paths::intercom_dir_path(agent_dir))
}

/// `getBrokerConnectTarget` (`paths.ts:76-105`), with `intercomDir` supplied explicitly (upstream
/// defaults it to `getIntercomDirPath(getAgentDirPath(env))`, `paths.ts:79`).
///
/// When the Windows TCP transport is active this reads and **validates** `broker.port.json`;
/// otherwise it is exactly [`broker_socket_path_for`].
///
/// # Errors
/// [`IntercomError::Io`] if `broker.port.json` cannot be read (pi's `readFileSync` throws — and
/// every caller either rejects the connect, `client.ts:171-176`, or treats it as "not connectable",
/// `spawn.ts:268-273`). [`IntercomError::Protocol`] if it is not valid JSON.
/// [`IntercomError::Broker`] carrying pi's exact
/// `Invalid intercom TCP endpoint at <file>: expected a JSON object` / `Invalid intercom TCP
/// endpoint at <file>` text if the endpoint fails validation (`paths.ts:85-100`).
pub fn broker_connect_target_in(
    platform: Platform,
    env: impl Fn(&str) -> Option<String>,
    agent_dir: &Path,
    intercom_dir: &Path,
) -> Result<BrokerConnectTarget> {
    if should_use_windows_tcp_transport(platform, env) {
        let endpoint_file = broker_port_file_path(intercom_dir);
        let raw = std::fs::read_to_string(&endpoint_file)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)?;
        return parse_tcp_endpoint(&parsed, &endpoint_file).map(BrokerConnectTarget::Tcp);
    }
    Ok(BrokerConnectTarget::Socket(broker_socket_path_for(
        platform, agent_dir,
    )))
}

/// [`broker_connect_target_in`] with `intercom_dir` derived from `agent_dir`, mirroring upstream's
/// `intercomDir = getIntercomDirPath(getAgentDirPath(env))` default (`paths.ts:79`).
///
/// # Errors
/// Same as [`broker_connect_target_in`].
pub fn broker_connect_target_from(
    platform: Platform,
    env: impl Fn(&str) -> Option<String>,
    agent_dir: &Path,
) -> Result<BrokerConnectTarget> {
    let intercom_dir = crate::paths::intercom_dir_path(agent_dir);
    broker_connect_target_in(platform, env, agent_dir, &intercom_dir)
}

/// [`broker_connect_target_from`] against the real platform + process env (pi's default-parameter
/// call form, `getBrokerConnectTarget()`).
///
/// # Errors
/// Same as [`broker_connect_target_in`].
pub fn broker_connect_target(agent_dir: &Path) -> Result<BrokerConnectTarget> {
    broker_connect_target_from(Platform::current(), |k| std::env::var(k).ok(), agent_dir)
}

/// `getBrokerListenTarget` (`paths.ts:107-116`): `127.0.0.1:0` (bind-any-port) when the Windows TCP
/// transport is active, else the socket/pipe path the broker binds directly.
#[must_use]
pub fn broker_listen_target_from(
    platform: Platform,
    env: impl Fn(&str) -> Option<String>,
    agent_dir: &Path,
) -> BrokerConnectTarget {
    if should_use_windows_tcp_transport(platform, env) {
        return BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port: 0,
            state_id: None,
        });
    }
    BrokerConnectTarget::Socket(broker_socket_path_for(platform, agent_dir))
}

/// [`broker_listen_target_from`] against the real platform + process env.
#[must_use]
pub fn broker_listen_target(agent_dir: &Path) -> BrokerConnectTarget {
    broker_listen_target_from(Platform::current(), |k| std::env::var(k).ok(), agent_dir)
}

/// The `broker.port.json` validation ladder (`paths.ts:85-100`), error text byte-for-byte.
fn parse_tcp_endpoint(
    parsed: &serde_json::Value,
    endpoint_file: &Path,
) -> Result<BrokerTcpEndpoint> {
    // `typeof parsed !== "object" || parsed === null || Array.isArray(parsed)` (paths.ts:85).
    let Some(endpoint) = parsed.as_object() else {
        return Err(IntercomError::Broker(format!(
            "Invalid intercom TCP endpoint at {}: expected a JSON object",
            endpoint_file.display()
        )));
    };
    let invalid = || {
        IntercomError::Broker(format!(
            "Invalid intercom TCP endpoint at {}",
            endpoint_file.display()
        ))
    };

    if endpoint
        .get("transport")
        .and_then(serde_json::Value::as_str)
        != Some("tcp")
    {
        return Err(invalid());
    }
    // `endpoint.host !== INTERCOM_TCP_HOST` (paths.ts:91) — loopback only; a routable host is a
    // hard reject, not a warning.
    let Some(host) = endpoint.get("host").and_then(serde_json::Value::as_str) else {
        return Err(invalid());
    };
    if host != INTERCOM_TCP_HOST {
        return Err(invalid());
    }
    // `typeof port !== "number" || !Number.isSafeInteger(port) || port <= 0 || port > 65535`
    // (paths.ts:92-95). JSON numbers written as `41234.0` are integral `Number`s to JS, so an
    // integral f64 is accepted here too; a fractional one is not.
    let port = match endpoint.get("port") {
        Some(serde_json::Value::Number(n)) => {
            let as_int = n.as_u64().or_else(|| {
                n.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            });
            match as_int {
                Some(p) if p > 0 && p <= 65535 => u16::try_from(p).map_err(|_| invalid())?,
                _ => return Err(invalid()),
            }
        }
        _ => return Err(invalid()),
    };
    // `typeof stateId !== "string" || stateId.length === 0` (paths.ts:96-97).
    let state_id = match endpoint.get("stateId").and_then(serde_json::Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Err(invalid()),
    };

    Ok(BrokerTcpEndpoint {
        host: host.to_string(),
        port,
        state_id: Some(state_id),
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;

    /// The env-closure form of pi's plain `{ ... }` env object literal.
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key: &str| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// `paths.test.ts:41-45` — "getBrokerSocketPath uses named pipe on Windows".
    #[test]
    fn socket_path_uses_named_pipe_on_windows() {
        let pipe = broker_socket_path_for(Platform::Windows, Path::new("C:/Users/rcroh/.cyrup"));
        let pipe = pipe.to_string_lossy().to_string();
        assert!(pipe.starts_with(r"\\.\pipe\pi-intercom-"), "{pipe}");
        assert!(!pipe.ends_with("broker.sock"), "{pipe}");
        // The sanitizer collapses every non-alphanumeric run to a single `-` and lowercases.
        assert_eq!(pipe, r"\\.\pipe\pi-intercom-c-users-rcroh-cyrup");
    }

    /// `paths.ts:20-25` — a segment with nothing alphanumeric falls back to `"default"`, and
    /// leading/trailing separator runs are trimmed rather than becoming `-`.
    #[test]
    fn pipe_segment_sanitizer_matches_upstream_regex_chain() {
        assert_eq!(sanitize_pipe_segment("///"), "default");
        assert_eq!(sanitize_pipe_segment(""), "default");
        assert_eq!(sanitize_pipe_segment("/a//b/"), "a-b");
        assert_eq!(sanitize_pipe_segment("C:\\Users\\Ada"), "c-users-ada");
    }

    /// `paths.test.ts:47-50` — "getBrokerSocketPath uses broker.sock ... on non-Windows".
    #[test]
    fn socket_path_uses_broker_sock_on_non_windows() {
        assert_eq!(
            broker_socket_path_for(Platform::Unix, Path::new("/tmp/cyrup-agent")),
            PathBuf::from("/tmp/cyrup-agent/intercom/broker.sock")
        );
    }

    /// `paths.test.ts:52-57` — "Windows TCP transport is opt-in", case-for-case.
    #[test]
    fn windows_tcp_transport_is_opt_in() {
        assert!(!should_use_windows_tcp_transport(Platform::Windows, no_env));
        assert!(should_use_windows_tcp_transport(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")])
        ));
        assert!(should_use_windows_tcp_transport(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TCP, "1")])
        ));
        // MIRROR (stays green): the same opt-in on a non-Windows platform must NOT select TCP —
        // `paths.ts:48-50` returns false before ever looking at the env.
        assert!(!should_use_windows_tcp_transport(
            Platform::Unix,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")])
        ));
        assert!(!should_use_windows_tcp_transport(
            Platform::Unix,
            env_of(&[(ENV_INTERCOM_TCP, "1")])
        ));
    }

    /// `paths.ts:52,57-58` — both gates `.trim().toLowerCase()`, and the legacy var accepts only
    /// `1`/`true` (not, say, `yes`).
    #[test]
    fn transport_opt_in_is_trimmed_and_case_insensitive() {
        assert!(should_use_windows_tcp_transport(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "  TCP  ")])
        ));
        assert!(should_use_windows_tcp_transport(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TCP, " TRUE ")])
        ));
        assert!(!should_use_windows_tcp_transport(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TCP, "yes")])
        ));
        assert!(!should_use_windows_tcp_transport(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "pipe")])
        ));
    }

    /// `paths.test.ts:59-65` — "getBrokerListenTarget uses dynamic localhost TCP only when opted in
    /// on Windows".
    #[test]
    fn listen_target_is_dynamic_loopback_tcp_only_when_opted_in_on_windows() {
        let agent_dir = Path::new("C:/agent");
        assert_eq!(
            broker_listen_target_from(
                Platform::Windows,
                env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
                agent_dir
            ),
            BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
                host: INTERCOM_TCP_HOST.to_string(),
                port: 0,
                state_id: None,
            })
        );
        assert_eq!(
            broker_listen_target_from(Platform::Windows, no_env, agent_dir),
            BrokerConnectTarget::Socket(broker_socket_path_for(Platform::Windows, agent_dir))
        );
    }

    /// `paths.test.ts:67-88` — "getBrokerConnectTarget reads opt-in Windows TCP endpoint from
    /// intercom state", using pi's own fixture values verbatim.
    #[test]
    fn connect_target_reads_opt_in_tcp_endpoint_from_intercom_state() {
        let root = tempfile::tempdir().unwrap();
        let intercom_dir = root.path().join("intercom");
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(
            broker_port_file_path(&intercom_dir),
            r#"{"transport":"tcp","host":"127.0.0.1","port":41234,"stateId":"state-1"}"#,
        )
        .unwrap();

        let target = broker_connect_target_in(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
            root.path(),
            &intercom_dir,
        )
        .expect("a valid endpoint file resolves");
        assert_eq!(
            target,
            BrokerConnectTarget::Tcp(BrokerTcpEndpoint {
                host: "127.0.0.1".to_string(),
                port: 41234,
                state_id: Some("state-1".to_string()),
            })
        );
        assert_eq!(target.state_id(), Some("state-1"));
        // `broker.ts:196`: a TCP peer is never `trustedLocal`.
        assert!(!target.is_trusted_local(Platform::Windows));
    }

    /// `paths.test.ts:90-111` — "getBrokerConnectTarget rejects non-local TCP endpoint hosts".
    #[test]
    fn connect_target_rejects_non_local_tcp_endpoint_hosts() {
        let root = tempfile::tempdir().unwrap();
        let intercom_dir = root.path().join("intercom");
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(
            broker_port_file_path(&intercom_dir),
            r#"{"transport":"tcp","host":"10.0.0.5","port":41234,"stateId":"state-1"}"#,
        )
        .unwrap();

        let err = broker_connect_target_in(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
            root.path(),
            &intercom_dir,
        )
        .expect_err("a routable host must be rejected");
        assert!(
            err.to_string().contains("Invalid intercom TCP endpoint"),
            "pi's exact reject text (paths.ts:99): {err}"
        );
    }

    /// `paths.ts:89-98` — the rest of the validation ladder, one reject per clause.
    #[test]
    fn connect_target_rejects_each_invalid_endpoint_field() {
        let root = tempfile::tempdir().unwrap();
        let intercom_dir = root.path().join("intercom");
        std::fs::create_dir_all(&intercom_dir).unwrap();
        let port_file = broker_port_file_path(&intercom_dir);

        let cases: &[(&str, &str)] = &[
            (
                "wrong transport",
                r#"{"transport":"pipe","host":"127.0.0.1","port":1,"stateId":"s"}"#,
            ),
            (
                "port 0",
                r#"{"transport":"tcp","host":"127.0.0.1","port":0,"stateId":"s"}"#,
            ),
            (
                "port too large",
                r#"{"transport":"tcp","host":"127.0.0.1","port":65536,"stateId":"s"}"#,
            ),
            (
                "port not an integer",
                r#"{"transport":"tcp","host":"127.0.0.1","port":1.5,"stateId":"s"}"#,
            ),
            (
                "port as string",
                r#"{"transport":"tcp","host":"127.0.0.1","port":"41234","stateId":"s"}"#,
            ),
            (
                "missing stateId",
                r#"{"transport":"tcp","host":"127.0.0.1","port":41234}"#,
            ),
            (
                "empty stateId",
                r#"{"transport":"tcp","host":"127.0.0.1","port":41234,"stateId":""}"#,
            ),
        ];
        for (label, body) in cases {
            std::fs::write(&port_file, body).unwrap();
            let result = broker_connect_target_in(
                Platform::Windows,
                env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
                root.path(),
                &intercom_dir,
            );
            let Err(err) = result else {
                panic!("{label}: expected `Invalid intercom TCP endpoint`, got {result:?}");
            };
            assert!(
                err.to_string().contains("Invalid intercom TCP endpoint"),
                "{label}: {err}"
            );
        }

        // MIRROR (stays green): the same ladder accepts the one well-formed body.
        std::fs::write(
            &port_file,
            r#"{"transport":"tcp","host":"127.0.0.1","port":65535,"stateId":"s"}"#,
        )
        .unwrap();
        assert!(
            broker_connect_target_in(
                Platform::Windows,
                env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
                root.path(),
                &intercom_dir,
            )
            .is_ok(),
            "port 65535 is the inclusive upper bound (paths.ts:95)"
        );
    }

    /// `paths.ts:85-87` — a non-object JSON body gets the *distinct* `expected a JSON object`
    /// message, not the generic one.
    #[test]
    fn connect_target_rejects_non_object_endpoint_with_distinct_message() {
        let root = tempfile::tempdir().unwrap();
        let intercom_dir = root.path().join("intercom");
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(broker_port_file_path(&intercom_dir), "[1,2,3]").unwrap();

        let err = broker_connect_target_in(
            Platform::Windows,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
            root.path(),
            &intercom_dir,
        )
        .expect_err("an array is not a JSON object");
        assert!(
            err.to_string().contains("expected a JSON object"),
            "pi's exact reject text (paths.ts:86): {err}"
        );
    }

    /// MIRROR (stays green): with no opt-in — and on the platform this suite actually runs on — the
    /// connect target is the plain Unix socket, and the endpoint file is not even consulted.
    #[test]
    fn connect_target_is_the_unix_socket_without_opt_in_even_when_a_port_file_exists() {
        let root = tempfile::tempdir().unwrap();
        let intercom_dir = crate::paths::intercom_dir_path(root.path());
        std::fs::create_dir_all(&intercom_dir).unwrap();
        std::fs::write(
            broker_port_file_path(&intercom_dir),
            r#"{"transport":"tcp","host":"127.0.0.1","port":41234,"stateId":"state-1"}"#,
        )
        .unwrap();

        let target = broker_connect_target_from(
            Platform::Unix,
            env_of(&[(ENV_INTERCOM_TRANSPORT, "tcp")]),
            root.path(),
        )
        .expect("the unix branch never fails");
        assert_eq!(
            target,
            BrokerConnectTarget::Socket(intercom_dir.join("broker.sock"))
        );
        assert_eq!(
            target.state_id(),
            None,
            "a socket target never carries a stateId"
        );
        assert!(target.is_trusted_local(Platform::Unix), "broker.ts:196");
    }

    /// `broker.ts:73-79` — the exact `broker.port.json` body, so a cyrup broker and a pi client (or
    /// vice versa) agree on the file format. Round-trips through the validator.
    #[test]
    fn port_file_body_round_trips_through_the_validator() {
        let endpoint = BrokerTcpEndpoint {
            host: INTERCOM_TCP_HOST.to_string(),
            port: 41234,
            state_id: Some("state-1".to_string()),
        };
        let body = endpoint.to_port_file_body();
        assert!(
            body.ends_with('\n'),
            "broker.ts:79 writes a trailing newline"
        );
        let value: serde_json::Value = serde_json::from_str(body.trim_end()).unwrap();
        assert_eq!(value["transport"], "tcp");
        assert_eq!(value["host"], "127.0.0.1");
        assert_eq!(value["port"], 41234);
        assert_eq!(value["stateId"], "state-1");
        assert_eq!(
            parse_tcp_endpoint(&value, Path::new("/x")).unwrap(),
            endpoint
        );
    }
}
