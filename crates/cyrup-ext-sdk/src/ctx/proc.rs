//! The `proc` WIT import (R-08-030-adjacent; arch-08 §5.2 request/poll bridge,
//! pi-mcp-adapter-port.md §3.1). A long-lived, duplex-pipe child process — MCP stdio transport's
//! shape. Distinct from [`Ctx::exec`]'s bounded run-capture-return one-shot: a `proc` handle stays
//! open across many calls, with a live stdin the guest keeps writing to and a live stdout/stderr it
//! polls incrementally. Gated by the SAME trust check as `exec`/`http_request`.

use super::Ctx;

impl Ctx {
    /// Spawn a long-lived child (the `proc.spawn` capability grant); returns an opaque handle for
    /// [`Ctx::proc_write_stdin`]/[`Ctx::proc_read_stdout`]/[`Ctx::proc_read_stderr`]/
    /// [`Ctx::proc_poll_exit`]/[`Ctx::proc_kill`]. Denied unless the host granted `proc`.
    pub fn proc_spawn(
        &self,
        cmd: &str,
        args: &[&str],
        opts: &ProcSpawnOptions,
    ) -> Result<u32, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let env_json = opts.env_json();
            return crate::guest::bindings::cyrup::ext::proc::spawn(
                cmd,
                &argv,
                &env_json,
                opts.cwd.as_deref(),
                opts.capture_stderr,
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (cmd, args, opts);
            Err("proc unavailable on host target".into())
        }
    }

    /// Write to a spawned child's REAL stdin (the `proc.write-stdin` capability grant); returns
    /// bytes written, or `Err` once the pipe is closed.
    pub fn proc_write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::proc::write_stdin(handle, data);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (handle, data);
            Err("proc unavailable on host target".into())
        }
    }

    /// Drain currently-buffered stdout (the `proc.read-stdout` capability grant; non-blocking poll —
    /// an empty result means "no data yet", NOT EOF; poll [`Ctx::proc_poll_exit`] for real EOF).
    pub fn proc_read_stdout(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::proc::read_stdout(handle, max_bytes);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (handle, max_bytes);
            Err("proc unavailable on host target".into())
        }
    }

    /// Drain currently-buffered stderr (the `proc.read-stderr` capability grant). Mirrors
    /// [`Ctx::proc_read_stdout`]; permanently empty (never an error) if the child was spawned with
    /// [`ProcSpawnOptions::capture_stderr`] unset.
    pub fn proc_read_stderr(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::proc::read_stderr(handle, max_bytes);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (handle, max_bytes);
            Err("proc unavailable on host target".into())
        }
    }

    /// Poll whether a spawned child has exited (the `proc.poll-exit` capability grant);
    /// `Some(code)` once terminated, `None` while still running.
    pub fn proc_poll_exit(&self, handle: u32) -> Option<i32> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::proc::poll_exit(handle);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = handle;
            None
        }
    }

    /// Terminate a spawned child — SIGTERM, then SIGKILL after a host-policy grace period if still
    /// alive (the `proc.kill` capability grant). Resolves only once the process is confirmed gone.
    pub fn proc_kill(&self, handle: u32) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::proc::kill(handle);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = handle;
            Err("proc unavailable on host target".into())
        }
    }
}

/// Options for [`Ctx::proc_spawn`] (mirrors the WIT `proc.spawn` params, minus `cmd`/`args`).
/// `env` is OVERLAID onto the host's own inherited environment, never a full replacement (Pi
/// `resolveEnv`, `server-manager.ts:422`). `capture_stderr` mirrors Pi's debug-mode "inherit" vs
/// "ignore" (`server-manager.ts:111`): unset (`false`) means the child's stderr is dropped, not
/// surfaced via [`Ctx::proc_read_stderr`].
#[derive(Clone, Debug, Default)]
pub struct ProcSpawnOptions {
    /// Extra environment pairs, OVERLAID onto the host's own inherited environment rather than
    /// replacing it (see the type doc above). Built with [`Self::env`].
    pub env: Vec<(String, String)>,
    /// The child's working directory; `None` leaves the choice to the host. Built with
    /// [`Self::cwd`].
    pub cwd: Option<String>,
    /// Pipe and buffer the child's stderr so [`Ctx::proc_read_stderr`] can drain it; left unset
    /// (`false`) the child's stderr is dropped. Built with [`Self::capture_stderr`].
    pub capture_stderr: bool,
}

impl ProcSpawnOptions {
    /// Append (or override) an environment variable (builder-style).
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
    /// Set the child's working directory (builder-style).
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    /// Pipe + buffer stderr for [`Ctx::proc_read_stderr`] (builder-style).
    #[must_use]
    pub fn capture_stderr(mut self, yes: bool) -> Self {
        self.capture_stderr = yes;
        self
    }

    /// The `env` pairs as a JSON object.
    ///
    /// **On an encode failure the env map is replaced with `{}`** and the child is spawned with no
    /// extra environment. Nothing author-supplied is serialized here — the value is a
    /// `HashMap<&str, &str>` borrowed from the already-owned `env` `String`s, and a string-keyed
    /// map of strings has no `serde_json` failure mode — so the substitution is unreachable.
    #[cfg(target_arch = "wasm32")]
    fn env_json(&self) -> String {
        // Scoped to the body: `env_json` is the only `HashMap` user here and it is wasm-only, so a
        // file-scope `use` would be an unused import on the host target.
        use std::collections::HashMap;

        let map: HashMap<&str, &str> = self
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        // Encode failure -> `{}` (unreachable, see the doc comment above).
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
    }
}
