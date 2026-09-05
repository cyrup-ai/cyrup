//! The `host-bash` WIT import: the one command a guest-supplied bash backend was asked to run, and
//! the two closure-shaped options pi passes alongside it (DRIFT-004).
//!
//! Upstream a backend is an object with one method and its options bag is
//! `{onData, signal, timeout, env}` (`packages/coding-agent/src/core/tools/bash.ts:71-80`
//! @v0.84.4). A closure cannot cross a component boundary (ADR-0002), so the host keys the call and
//! offers the two closures back as imports: [`BashCommand::write`] is pi's `onData` and
//! [`BashCommand::is_cancelled`] is its `signal.aborted`.

/// The command a guest [`BashOperations`](crate::api::BashOperations) backend must run — pi's
/// `exec(command, cwd, options)` arguments, flattened (`core/tools/bash.ts:71-80` @v0.84.4).
///
/// The backend streams output with [`Self::write`] as it arrives (pi's `onData`), polls
/// [`Self::is_cancelled`] while it waits (pi's `signal`), and returns the exit code — or `None` for
/// pi's `exitCode: null`, "killed".
#[derive(Clone, Debug)]
pub struct BashCommand {
    /// The host's key for this command — the id [`Self::write`] streams against and
    /// [`Self::is_cancelled`] polls. Not pi's, which needs no key: its `onData`/`signal` ARE the
    /// call.
    pub call_id: String,
    /// The shell command line to run (pi `command`).
    pub command: String,
    /// The working directory to run it in (pi `cwd`).
    pub cwd: String,
    /// pi `timeout?: number`, in MILLISECONDS here rather than upstream's seconds — the
    /// seconds→ms conversion and the `MAX_TIMEOUT_MS` ceiling are the bash TOOL's input validation
    /// (`core/tools/bash.ts:28-40`), already applied by the time a backend sees it. `None` is pi's
    /// absent `timeout`: no timeout at all.
    ///
    /// The host enforces it too (it stops waiting and reports a timeout), so a backend that ignores
    /// it cannot hold the command open — but a backend that honours it is the one that gets to kill
    /// its own child, which is what upstream's local backend does (`:119-123`).
    pub timeout_ms: Option<u64>,
    /// pi `env?: NodeJS.ProcessEnv`, as ADDITIVE key/value pairs over the inherited shell
    /// environment. Upstream materializes the whole environment and can therefore express a
    /// deletion by omission; cyrup inherits, so deletions are named separately in
    /// [`Self::env_remove`].
    pub env: Vec<(String, String)>,
    /// Keys to UNSET before running, applied BEFORE [`Self::env`] — see that field.
    pub env_remove: Vec<String>,
}

impl BashCommand {
    /// Decode the host's `bash-operations-exec` arguments — the three flat strings plus the
    /// `opts-json` bag `{timeoutMs, env, envRemove}` the host builds from
    /// `cyrup_tools::ops::BashExecOptions`.
    ///
    /// Every field degrades to its empty/absent form rather than failing the command: a malformed
    /// options bag must not turn a runnable command into an error, exactly as the rest of this
    /// SDK's seam decoding treats unparseable host JSON as `Null` and carries on.
    pub fn from_host_args(
        call_id: impl Into<String>,
        command: impl Into<String>,
        cwd: impl Into<String>,
        opts_json: &str,
    ) -> Self {
        let opts: serde_json::Value =
            serde_json::from_str(opts_json).unwrap_or(serde_json::Value::Null);
        Self {
            call_id: call_id.into(),
            command: command.into(),
            cwd: cwd.into(),
            timeout_ms: opts.get("timeoutMs").and_then(serde_json::Value::as_u64),
            env: opts
                .get("env")
                .and_then(serde_json::Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str().map(str::to_string).unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            env_remove: opts
                .get("envRemove")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Stream one raw output chunk back to the host (pi `onData(data: Buffer)`).
    ///
    /// RAW bytes, combined stdout+stderr: the host sanitizes (strips ANSI, normalizes CR) and
    /// buffers, exactly as `executeBashWithOperations` does inside its own `onData` wrapper
    /// (`core/bash-executor.ts:78-105` @v0.84.4) — a backend that pre-sanitizes would double the
    /// work and lose bytes the host's truncation accounting expects to see.
    pub fn write(&self, chunk: &[u8]) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::host_bash::emit_bash_output(&self.call_id, chunk);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = chunk;
    }

    /// Whether the host has cancelled this command (pi `signal.aborted`). A backend blocked on a
    /// remote command polls this and stops cooperatively; the host's own cancellation is the
    /// backstop and does not depend on the poll.
    pub fn is_cancelled(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::host_bash::is_bash_cancelled(&self.call_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }
}
