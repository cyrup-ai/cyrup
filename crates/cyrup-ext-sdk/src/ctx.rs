//! The handler context wrappers (arch-08 §2.2/§6.3; Pi `ExtensionContext`/`ExtensionUIContext`/
//! `ExtensionCommandContext`, types.ts:124-390). Every event/tool handler receives a [`Ctx`], the
//! safe-Rust front for the `ui`/`session`/`models`/`exec`/`bus` capability imports. Command handlers
//! receive a [`CommandCtx`] which additionally exposes the COMMAND-only `control` ops — the
//! type-level half of the deadlock rule (the host check is authoritative, R-08-008).
//!
//! On `wasm32` each method calls the generated WIT import; on the host target (unit tests) the
//! methods return inert defaults so the ergonomic API is exercisable without a runtime.
//!
//! `needless_return` is allowed: the `#[cfg]`-split dual bodies use an early `return` in the wasm
//! arm so the host arm can be a distinct tail expression.
#![allow(clippy::needless_return)]

use crate::descriptor::{
    CompactOptions, DialogOptions, ExecOptions, ForkOptions, NavigateOptions, NewSessionOptions,
    SwitchSessionOptions,
};
use core::cell::RefCell;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The mode the host is running in (Pi `ExtensionMode`, types.ts:305 — `"tui" | "rpc" | "json" |
/// "print"`); the WIT `types.ext-mode` enum. Mirrored here rather than re-exported from the
/// generated bindings so the type also exists when the SDK is compiled for the host target (unit
/// tests), where the bindings module does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExtMode {
    #[default]
    Tui,
    Rpc,
    Json,
    Print,
}

impl ExtMode {
    /// The Pi wire spelling (`ExtensionMode`, types.ts:305).
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtMode::Tui => "tui",
            ExtMode::Rpc => "rpc",
            ExtMode::Json => "json",
            ExtMode::Print => "print",
        }
    }
}

/// The capability context handed to every handler (event tier: no session mutation).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ctx;

impl Ctx {
    pub fn new() -> Self {
        Ctx
    }
    /// UI surface (R-08-022).
    pub fn ui(&self) -> Ui {
        Ui
    }
    /// Read-only session view + state persistence (R-08-026/027).
    pub fn session(&self) -> Session {
        Session
    }
    /// Model registry view (read; `set_model` is command-only).
    pub fn models(&self) -> Models {
        Models
    }

    /// Convenience: post a transient notification.
    pub fn notify(&self, message: &str) {
        self.ui().notify(message);
    }

    /// Emit on the inter-extension event bus (R-08-029).
    pub fn emit(&self, topic: &str, payload: impl Serialize) {
        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::bus::emit(topic, &payload);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (topic, payload);
        }
    }

    /// Register a tool from inside a LIVE handler, after `init` (Pi `api.registerTool()` called at
    /// runtime — `examples/extensions/dynamic-tools.ts` registers from a `session_start` handler;
    /// `extensions/loader.ts:249-256` follows every registration with `runtime.refreshTools()`).
    /// The host re-materializes it into an executable handle at its next tool refresh, so the tool
    /// is model-visible on the following turn.
    pub fn register_tool(
        &self,
        descriptor: crate::descriptor::ToolDescriptor,
        exec: impl crate::api::ToolExec,
    ) {
        let tool = crate::api::RegisteredTool { descriptor, exec: Box::new(exec) };
        #[cfg(target_arch = "wasm32")]
        crate::guest::register_tool_late(tool);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = tool;
    }

    // --- base-context state + lifecycle (Pi `ExtensionContext`, types.ts:305-347). Pi puts ALL of
    // these on the base context — "Available in all contexts" — so they live on `Ctx`, not on
    // `CommandCtx`, and the host does not tier-gate them (EXT-005). ---

    /// The mode the host is running in (Pi `ctx.mode`, types.ts:311). Pi's guidance: "Use `tui` to
    /// guard terminal-only UI such as custom components" — a widget or a custom renderer is
    /// meaningless under `json`/`print`, so branch on this before registering one.
    pub fn mode(&self) -> ExtMode {
        #[cfg(target_arch = "wasm32")]
        {
            use crate::guest::bindings::cyrup::ext::types::ExtMode as Wit;
            return match crate::guest::bindings::cyrup::ext::ctx_state::get_mode() {
                Wit::Tui => ExtMode::Tui,
                Wit::Rpc => ExtMode::Rpc,
                Wit::Json => ExtMode::Json,
                Wit::Print => ExtMode::Print,
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        ExtMode::Tui
    }

    /// Whether dialog-capable UI is available (Pi `ctx.hasUI`, types.ts:313 — "true in TUI and RPC
    /// modes"). Check this before [`Ui::confirm`]/[`Ui::input`]/[`Ui::select`]: with no UI those
    /// answer with their inert default rather than reaching a human.
    pub fn has_ui(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::has_ui();
        }
        #[cfg(not(target_arch = "wasm32"))]
        true
    }

    /// Whether no agent run is in flight (Pi `ctx.isIdle()`, types.ts:333).
    pub fn is_idle(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::is_idle();
        }
        #[cfg(not(target_arch = "wasm32"))]
        true
    }

    /// Whether user messages are queued for the next turn (Pi `ctx.hasPendingMessages()`,
    /// types.ts:341).
    pub fn has_pending_messages(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::has_pending_messages();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }

    /// Whether the project is trusted (Pi `ctx.isProjectTrusted()`, types.ts:335).
    pub fn is_project_trusted(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::is_project_trusted();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }

    /// The active system prompt (Pi `ctx.getSystemPrompt()`, types.ts:346); empty when no session
    /// backend is attached.
    pub fn system_prompt(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::get_system_prompt();
        }
        #[cfg(not(target_arch = "wasm32"))]
        String::new()
    }

    /// Abort the in-flight agent run (Pi `ctx.abort()`, types.ts:339 — available in all contexts).
    pub fn abort(&self) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::control::abort();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Ok(())
    }

    /// Request a graceful host shutdown (Pi `ctx.shutdown()`, types.ts:344 — available in all
    /// contexts). The host exits at its next settle point.
    pub fn shutdown(&self) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::control::shutdown();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Ok(())
    }

    // --- active-tool / command introspection (Pi getActiveTools/…/getCommands, types.ts:1257-1266) ---

    /// The names of the currently-active tools (Pi `getActiveTools`).
    pub fn get_active_tools(&self) -> Vec<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_active_tools())
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Vec::new()
    }
    /// All configured tools with metadata (Pi `getAllTools` → `ToolInfo[]`).
    pub fn get_all_tools(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_all_tools());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    /// Restrict the active tool set by name (Pi `setActiveTools`; plan-mode-style restriction).
    pub fn set_active_tools(&self, names: &[&str]) {
        let names_json = serde_json::to_string(names).unwrap_or_else(|_| "[]".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ext_tools::set_active_tools(&names_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = names_json;
    }
    /// Available slash commands (Pi `getCommands` → `SlashCommandInfo[]`).
    pub fn get_commands(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_commands());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    /// Read a registered flag's resolved VALUE (Pi `getFlag(name)`, types.ts:1218; sdk gap #23). The
    /// WIT `registration.get-flag` import returns the value (its default / CLI override) as JSON; this
    /// wraps it. `None` when the flag is unregistered or has no value (Pi `undefined`).
    pub fn get_flag(&self, name: &str) -> Option<Value> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::registration::get_flag(name)
                .and_then(|s| serde_json::from_str(&s).ok());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = name;
            None
        }
    }

    /// Unregister a custom provider previously registered by this extension (Pi `unregisterProvider`,
    /// types.ts:1361; sdk gap #24). Wraps the existing WIT `registration.unregister-provider` import.
    pub fn unregister_provider(&self, id: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::registration::unregister_provider(id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = id;
    }

    /// Run a capability-scoped command (R-08-030). Denied unless the host granted the exec capability.
    pub fn exec(&self, cmd: &str, args: &[&str], opts: &ExecOptions) -> Result<ExecResult, String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            match crate::guest::bindings::cyrup::ext::exec::run(cmd, &argv, &opts_json) {
                Ok(r) => Ok(ExecResult {
                    code: r.code,
                    stdout: r.stdout,
                    stderr: r.stderr,
                    killed: r.killed,
                }),
                Err(e) => Err(e),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (cmd, args, opts_json);
            Err("exec unavailable on host target".into())
        }
    }

    /// Read a file through the capability-scoped `ext-fs` grant (EXT-055). `path` is relative to the
    /// project root; the host resolves it against the `capabilities.fs` roots the extension's
    /// `extension.json` declared (`["read:.", "write:.cyrup/todo"]`) and refuses anything outside
    /// them — including a declaration-free manifest, which grants no root at all.
    ///
    /// Before EXT-054/EXT-055 the `ext-fs` interface had no SDK wrapper and no host-side root, so it
    /// was unreachable from a guest in both directions at once.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ext_fs::read_file(path);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = path;
            Err("ext-fs unavailable on host target".into())
        }
    }

    /// Write a file through the capability-scoped `ext-fs` grant (EXT-055). Requires a `write:`
    /// root in `capabilities.fs` covering `path` — a `read:` grant is refused, which is the whole
    /// point of the manifest having two modes.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ext_fs::write_file(path, data);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (path, data);
            Err("ext-fs unavailable on host target".into())
        }
    }

    /// A bounded outbound HTTP request/response round trip (the `http-client.request` capability
    /// grant; arch-08 §3.2 draft, pi-mcp-adapter-port.md §3.2). Gated by the SAME trust check as
    /// [`Ctx::exec`] — denied unless the host granted the http-client capability. A non-2xx status is
    /// NOT itself an `Err` (fetch semantics); inspect [`HttpResponse::status`].
    pub fn http_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let wit = req.to_wit();
            return crate::guest::bindings::cyrup::ext::http_client::request(&wit)
                .map(HttpResponse::from_wit);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = req;
            Err("http-client unavailable on host target".into())
        }
    }

    /// Start a streaming outbound HTTP request (the `http-client.request-stream` capability grant);
    /// returns the initiating response's status+headers TOGETHER with an opaque stream handle (the
    /// guest drains the body via [`Ctx::http_poll_stream_chunk`]) — the HOST owns the live Rust stream
    /// (a guest cannot hold one across the wasm boundary, arch-08 §5.2's request/poll bridge).
    /// Status/headers arrive off the SAME round trip that opens the body (closes L4 §2.3): the real
    /// consumer this backs, the MCP TS SDK's `StreamableHTTPClientTransport`/`SSEClientTransport`,
    /// reads `response.status` (401 => re-auth) and `response.headers` (`mcp-session-id`,
    /// `content-type`) off the SAME response whose body it then streams. Gated the same way as
    /// [`Ctx::http_request`].
    pub fn http_request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let wit = req.to_wit();
            return crate::guest::bindings::cyrup::ext::http_client::request_stream(&wit)
                .map(HttpStreamResponse::from_wit);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = req;
            Err("http-client unavailable on host target".into())
        }
    }

    /// Drain the next chunk of a stream opened via [`Ctx::http_request_stream`] (the
    /// `http-client.poll-stream-chunk` import); `Ok(None)` = EOF.
    pub fn http_poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::http_client::poll_stream_chunk(handle);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = handle;
            Err("http-client unavailable on host target".into())
        }
    }

    /// Close (drop/cancel) a stream opened via [`Ctx::http_request_stream`] (the
    /// `http-client.close-stream` import).
    pub fn http_close_stream(&self, handle: u32) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::http_client::close_stream(handle);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = handle;
    }

    // --- proc (R-08-030-adjacent; arch-08 §5.2 request/poll bridge, pi-mcp-adapter-port.md §3.1) ---
    // A long-lived, duplex-pipe child process — MCP stdio transport's shape. Distinct from
    // [`Ctx::exec`]'s bounded run-capture-return one-shot: a `proc` handle stays open across many
    // calls, with a live stdin the guest keeps writing to and a live stdout/stderr it polls
    // incrementally. Gated by the SAME trust check as `exec`/`http_request`.

    /// Spawn a long-lived child (the `proc.spawn` capability grant); returns an opaque handle for
    /// [`Ctx::proc_write_stdin`]/[`Ctx::proc_read_stdout`]/[`Ctx::proc_read_stderr`]/
    /// [`Ctx::proc_poll_exit`]/[`Ctx::proc_kill`]. Denied unless the host granted `proc`.
    pub fn proc_spawn(&self, cmd: &str, args: &[&str], opts: &ProcSpawnOptions) -> Result<u32, String> {
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

/// Result of [`Ctx::exec`] (Pi `ExecResult`, exec.ts:23-28). `killed` is true when the host
/// SIGTERMed, then (after a 5s grace period if still alive) SIGKILLed, the process GROUP on a
/// timeout/abort — Pi's exact `killProcess` escalation (exec.ts:52-63).
#[derive(Clone, Debug, Default)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub killed: bool,
}

/// An outbound HTTP request (`Ctx::http_request`/`http_request_stream`; mirrors the WIT
/// `http-request` record 1:1, arch-08 §3.2 draft, pi-mcp-adapter-port.md §3.2).
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: Option<u32>,
}

impl HttpRequest {
    /// A bare `GET` to `url`.
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: "GET".into(), url: url.into(), ..Default::default() }
    }
    /// A `method` request to `url`.
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self { method: method.into(), url: url.into(), ..Default::default() }
    }
    /// Append a request header (builder-style).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    /// Set the request body (builder-style).
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }
    /// Set a request timeout in milliseconds (builder-style).
    pub fn timeout_ms(mut self, ms: u32) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    #[cfg(target_arch = "wasm32")]
    fn to_wit(&self) -> crate::guest::bindings::cyrup::ext::http_client::HttpRequest {
        crate::guest::bindings::cyrup::ext::http_client::HttpRequest {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            body: self.body.clone(),
            timeout_ms: self.timeout_ms,
        }
    }
}

/// The response to an [`HttpRequest`] (mirrors the WIT `http-response` record 1:1).
#[derive(Clone, Debug, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    #[cfg(target_arch = "wasm32")]
    fn from_wit(wit: crate::guest::bindings::cyrup::ext::http_client::HttpResponse) -> Self {
        Self { status: wit.status, headers: wit.headers, body: wit.body }
    }
}

/// The initiating response's metadata for a stream opened via [`Ctx::http_request_stream`] (mirrors
/// the WIT `http-stream-response` record 1:1): status+headers arrive TOGETHER with the stream handle,
/// off the SAME round trip that opens the long-lived body, so callers can inspect
/// [`Self::status`]/[`Self::headers`] (e.g. 401 => re-auth, `mcp-session-id`) before or independent of
/// draining the body via [`Ctx::http_poll_stream_chunk`].
#[derive(Clone, Debug, Default)]
pub struct HttpStreamResponse {
    pub handle: u32,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl HttpStreamResponse {
    #[cfg(target_arch = "wasm32")]
    fn from_wit(wit: crate::guest::bindings::cyrup::ext::http_client::HttpStreamResponse) -> Self {
        Self { handle: wit.handle, status: wit.status, headers: wit.headers }
    }
}

/// Options for [`Ctx::proc_spawn`] (mirrors the WIT `proc.spawn` params, minus `cmd`/`args`).
/// `env` is OVERLAID onto the host's own inherited environment, never a full replacement (Pi
/// `resolveEnv`, `server-manager.ts:422`). `capture_stderr` mirrors Pi's debug-mode "inherit" vs
/// "ignore" (`server-manager.ts:111`): unset (`false`) means the child's stderr is dropped, not
/// surfaced via [`Ctx::proc_read_stderr`].
#[derive(Clone, Debug, Default)]
pub struct ProcSpawnOptions {
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub capture_stderr: bool,
}

impl ProcSpawnOptions {
    /// Append (or override) an environment variable (builder-style).
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
    /// Set the child's working directory (builder-style).
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    /// Pipe + buffer stderr for [`Ctx::proc_read_stderr`] (builder-style).
    pub fn capture_stderr(mut self, yes: bool) -> Self {
        self.capture_stderr = yes;
        self
    }

    #[cfg(target_arch = "wasm32")]
    fn env_json(&self) -> String {
        let map: HashMap<&str, &str> =
            self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
    }
}

/// Notification severity (Pi `notify` `type`: `"info" | "warning" | "error"`, types.ts:135).
/// [`NotifyKind::Info`] is Pi's default when the argument is omitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyKind {
    #[default]
    Info,
    Warning,
    Error,
}

#[cfg(target_arch = "wasm32")]
impl NotifyKind {
    fn to_wit(self) -> crate::guest::bindings::cyrup::ext::ui::NotifyKind {
        use crate::guest::bindings::cyrup::ext::ui::NotifyKind as Wit;
        match self {
            NotifyKind::Info => Wit::Info,
            NotifyKind::Warning => Wit::Warning,
            NotifyKind::Error => Wit::Error,
        }
    }
}

/// The UI capability surface (Pi `ExtensionUIContext`, types.ts:124-275).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ui;

impl Ui {
    /// Show an `info`-severity notification (Pi `notify(message)`; the `type` defaults to `"info"`).
    pub fn notify(&self, message: &str) {
        self.notify_with(message, NotifyKind::Info);
    }
    /// Show a notification with an explicit severity (Pi `notify(message, type)`, types.ts:135).
    pub fn notify_with(&self, message: &str, kind: NotifyKind) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::notify(message, kind.to_wit());
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (message, kind);
    }
    /// Set a keyed status segment (Pi `setStatus(key, text)`, types.ts:141). Pass [`None`] for
    /// `text` to clear that segment (Pi `setStatus(key, undefined)`).
    pub fn set_status(&self, key: &str, text: Option<&str>) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_status(key, text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (key, text);
    }
    /// Clear a keyed status segment (Pi `setStatus(key, undefined)`).
    pub fn clear_status(&self, key: &str) {
        self.set_status(key, None);
    }
    /// Programmatically dismiss any dialog bound to `signal_id` (Pi `ExtensionUIDialogOptions.signal`
    /// `AbortSignal.abort()`, types.ts:89-94; sdk gap #2). A dialog subsequently opened via
    /// [`Self::confirm_with`]/[`Self::input_with`]/[`Self::select_with`] carrying that signal id
    /// returns cancelled.
    pub fn abort_signal(&self, signal_id: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::abort_signal(signal_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = signal_id;
    }
    /// Confirmation dialog (Pi `confirm`). Indefinite, no message body; use [`Self::confirm_with`]
    /// for a message/timeout/signal.
    pub fn confirm(&self, prompt: &str) -> bool {
        self.confirm_with(prompt, "", &DialogOptions::default())
    }
    /// Confirmation dialog with a message body and a [`DialogOptions`] bag (Pi
    /// `confirm(title, message, {timeout, signal})`, rpc-types.ts:232): `prompt` is the short title,
    /// `message` the (often large, formatted) body — e.g. pi-mcp-adapter's sampling handler passes a
    /// label as `title` and the full prompt/conversation text as `message`.
    pub fn confirm_with(&self, prompt: &str, message: &str, opts: &DialogOptions) -> bool {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::confirm(prompt, message, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, message, opts_json);
            false
        }
    }
    /// Text input dialog (Pi `input`). No placeholder; use [`Self::input_with`] to set one.
    pub fn input(&self, prompt: &str) -> Option<String> {
        self.input_with(prompt, None, &DialogOptions::default())
    }
    /// Text input dialog with a placeholder and a [`DialogOptions`] bag (Pi
    /// `input(title, placeholder, {timeout, signal})`, rpc-types.ts:233-240); forwarded live to the
    /// renderer. `placeholder = None` omits the wire field entirely, matching Pi's optional field.
    pub fn input_with(&self, prompt: &str, placeholder: Option<&str>, opts: &DialogOptions) -> Option<String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::input(prompt, placeholder, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, placeholder, opts_json);
            None
        }
    }
    /// Single-choice select; returns the chosen option string (Pi `select(title, options, opts):
    /// Promise<string|undefined>`, types.ts:127).
    pub fn select(&self, prompt: &str, options: &[&str]) -> Option<String> {
        self.select_with(prompt, options, &DialogOptions::default())
    }
    /// Single-choice select with a [`DialogOptions`] bag (Pi `select(title, options, {timeout, signal})`).
    pub fn select_with(&self, prompt: &str, options: &[&str], opts: &DialogOptions) -> Option<String> {
        let options_json = serde_json::to_string(options).unwrap_or_else(|_| "[]".into());
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::select(prompt, &options_json, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, options_json, opts_json);
            None
        }
    }
    /// Multiline editor labeled `title`, seeded with `initial` (Pi `editor(title, prefill):
    /// Promise<string|undefined>`, types.ts:216); returns the edited text (None = cancelled).
    pub fn editor(&self, title: &str, initial: &str) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::editor(title, initial);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (title, initial);
            None
        }
    }
    pub fn set_widget(&self, widget: impl Serialize) {
        let widget_json = serde_json::to_string(&widget).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_widget(&widget_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = widget_json;
    }

    // --- chrome (Pi setHeader/setFooter/setTitle, types.ts:130-150) ---
    pub fn set_header(&self, content: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_header(content);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = content;
    }
    pub fn set_footer(&self, content: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_footer(content);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = content;
    }
    pub fn set_title(&self, title: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_title(title);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = title;
    }
    /// A custom overlay component; returns an optional serialized result (Pi `custom()`).
    pub fn custom(&self, spec: impl Serialize) -> Option<String> {
        let spec_json = serde_json::to_string(&spec).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::custom(&spec_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = spec_json;
            None
        }
    }

    // --- editor buffer access (Pi getEditorText/setEditorText/pasteEditorText, types.ts:200-230) ---
    pub fn editor_text(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::get_editor_text();
        }
        #[cfg(not(target_arch = "wasm32"))]
        String::new()
    }
    pub fn set_editor_text(&self, text: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_editor_text(text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = text;
    }
    pub fn paste_editor_text(&self, text: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::paste_editor_text(text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = text;
    }

    // --- theme get/list/set (Pi getTheme/listThemes/setTheme, types.ts:240-260) ---
    pub fn theme(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_get();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn theme_list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ui::theme_list());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    pub fn set_theme(&self, name: &str) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_set(name);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = name;
            Ok(())
        }
    }

    // --- working-indicator controls (Pi startWorking/stopWorking, types.ts:265-275) ---
    pub fn working_start(&self, label: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::working_start(label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = label;
    }
    pub fn working_stop(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::working_stop();
    }

    // --- tools-expanded get/set (Pi getToolsExpanded/setToolsExpanded) ---
    pub fn tools_expanded(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::get_tools_expanded();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }
    pub fn set_tools_expanded(&self, expanded: bool) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_tools_expanded(expanded);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = expanded;
    }
}

/// The read-only session view + state-persistence surface (Pi `ReadonlySessionManager` + R-08-026).
#[derive(Clone, Copy, Debug, Default)]
pub struct Session;

impl Session {
    pub fn entries(&self) -> Value {
        parse_json(session_call(SessionGet::Entries))
    }
    pub fn branch(&self) -> Value {
        parse_json(session_call(SessionGet::Branch))
    }
    pub fn tree(&self) -> Value {
        parse_json(session_call(SessionGet::Tree))
    }
    /// Persist a custom (non-LLM) entry (R-08-026); returns the new entry id.
    pub fn append_entry(&self, custom_type: &str, data: impl Serialize) -> Result<String, String> {
        let data_json = serde_json::to_string(&data).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::append_entry(custom_type, &data_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (custom_type, data_json);
            Err("append_entry unavailable on host target".into())
        }
    }
    pub fn session_name(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::get_session_name();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn set_session_name(&self, name: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::session::set_session_name(name);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = name;
    }
    pub fn set_label(&self, entry_id: &str, label: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::session::set_label(entry_id, label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (entry_id, label);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum SessionGet {
    Entries,
    Branch,
    Tree,
}

fn session_call(which: SessionGet) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::guest::bindings::cyrup::ext::session as s;
        return match which {
            SessionGet::Entries => s::entries_json(),
            SessionGet::Branch => s::branch_json(),
            SessionGet::Tree => s::tree_json(),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = which;
        "null".into()
    }
}

fn parse_json(s: String) -> Value {
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

/// The model registry view (Pi types.ts:1273-1279).
#[derive(Clone, Copy, Debug, Default)]
pub struct Models;

impl Models {
    pub fn list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::models::list_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    pub fn current(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::current();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn context_usage(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::models::context_usage());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Null
    }
    pub fn thinking_level(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::thinking_level();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// Set the thinking level (Pi `setThinkingLevel(level)`, types.ts:1288; sdk gap #25 / GAP-11).
    ///
    /// Pi allows `setThinkingLevel` from ANY handler (factory-tier `pi.*`, `loader.ts:352-354` /
    /// `runner.ts:330`, no tier gate) and it takes effect. cyrup now matches this: the call is QUEUED
    /// as a control op and applied at the store-free turn-boundary drain
    /// (`AgentSession::apply_pending_control`), so its `thinking_level_select` re-emit
    /// (`agent-session.ts:1560-1567`) runs as a fresh top-level guest call after the event hook's wasm
    /// store guard is released — never a re-entry into the suspended single-instance store (the
    /// R-08-008 deadlock the old command-tier gate guarded against is dissolved by deferral). So this
    /// returns `Ok(())` and the new level takes effect on the SUBSEQUENT turn, whether called from a
    /// command handler or an event handler.
    pub fn set_thinking_level(&self, level: &str) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::set_thinking_level(level);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = level;
            Ok(())
        }
    }
}

/// The command-tier context (Pi `ExtensionCommandContext`, types.ts:339-373). Adds the COMMAND-only
/// `control` ops to [`Ctx`]; the host rejects any control op from an event handler (R-08-008).
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandCtx {
    base: Ctx,
}

impl CommandCtx {
    pub fn new() -> Self {
        Self { base: Ctx }
    }
    pub fn ctx(&self) -> &Ctx {
        &self.base
    }
    pub fn ui(&self) -> Ui {
        self.base.ui()
    }
    pub fn session(&self) -> Session {
        self.base.session()
    }
    pub fn models(&self) -> Models {
        self.base.models()
    }

    pub fn new_session(&self) -> Result<(), String> {
        self.new_session_with(&NewSessionOptions::default())
    }
    /// Start a new session with typed options (Pi `newSession({parentSession, withSession})`).
    pub fn new_session_with(&self, opts: &NewSessionOptions) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::NewSession(&opts))
    }
    pub fn switch_session(&self, session_id: &str) -> Result<(), String> {
        self.switch_session_with(session_id, &SwitchSessionOptions::default())
    }
    /// Switch sessions with typed options (Pi `switchSession({withSession})`).
    pub fn switch_session_with(
        &self,
        session_id: &str,
        opts: &SwitchSessionOptions,
    ) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Switch(session_id, &opts))
    }
    pub fn fork(&self, entry_id: &str) -> Result<(), String> {
        self.fork_with(entry_id, &ForkOptions::default())
    }
    /// Fork with typed options (Pi `fork(entryId, {position, withSession})`).
    pub fn fork_with(&self, entry_id: &str, opts: &ForkOptions) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Fork(entry_id, &opts))
    }

    // --- withSession re-binding callbacks (Pi ReplacedSessionContext, types.ts:346-390; sdk gap #3) ---

    /// Start a new session, then run `with_session` against the re-bound session (Pi
    /// `newSession({withSession})`, types.ts:346). The closure is stored guest-side and invoked by the
    /// host's `with-session` export after the replacement completes and the command body returns —
    /// move post-replacement work here (Pi: a captured `ctx` is stale after `newSession`, runner.ts:511).
    pub fn new_session_with_callback(
        &self,
        opts: &NewSessionOptions,
        with_session: impl Fn(&ReplacedSessionContext) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let opts_json = opts_with_callback(opts, Box::new(with_session));
        control(Control::NewSession(&opts_json))
    }

    /// Fork, then run `with_session` against the re-bound session (Pi `fork(entryId, {withSession})`,
    /// types.ts:355).
    pub fn fork_with_callback(
        &self,
        entry_id: &str,
        opts: &ForkOptions,
        with_session: impl Fn(&ReplacedSessionContext) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let opts_json = opts_with_callback(opts, Box::new(with_session));
        control(Control::Fork(entry_id, &opts_json))
    }

    /// Switch sessions, then run `with_session` against the re-bound session (Pi
    /// `switchSession({withSession})`, types.ts:368).
    pub fn switch_session_with_callback(
        &self,
        session_id: &str,
        opts: &SwitchSessionOptions,
        with_session: impl Fn(&ReplacedSessionContext) -> Result<(), String> + 'static,
    ) -> Result<(), String> {
        let opts_json = opts_with_callback(opts, Box::new(with_session));
        control(Control::Switch(session_id, &opts_json))
    }
    pub fn navigate(&self, entry_id: &str, opts: impl Serialize) -> Result<(), String> {
        let opts = serde_json::to_string(&opts).unwrap_or_else(|_| "{}".into());
        control(Control::Navigate(entry_id, &opts))
    }
    /// Navigate the session tree with typed options (Pi `navigateTree(targetId, {summarize, …})`).
    pub fn navigate_with(&self, entry_id: &str, opts: &NavigateOptions) -> Result<(), String> {
        self.navigate(entry_id, opts)
    }
    pub fn reload(&self) -> Result<(), String> {
        control(Control::Reload)
    }
    /// Trigger a compaction with no extra guidance (Pi `ctx.compact()`, types.ts:344).
    pub fn compact(&self) -> Result<(), String> {
        self.compact_with(&CompactOptions::default())
    }

    /// Trigger a compaction with typed options (Pi `ctx.compact(options)`, types.ts:344 +
    /// `CompactOptions`, types.ts:296-300). `custom_instructions` reaches the summarizer that
    /// writes the compaction summary. Fire-and-forget, exactly as in Pi: the call returns once the
    /// host has queued the op — subscribe to the `session_compact` event for the result (see
    /// [`CompactOptions`] on why the `onComplete`/`onError` callbacks have no cross-boundary form).
    pub fn compact_with(&self, opts: &CompactOptions) -> Result<(), String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Compact(&opts_json))
    }
    pub fn wait_idle(&self) -> Result<(), String> {
        control(Control::WaitIdle)
    }
    pub fn set_model(&self, model: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&model).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            crate::guest::bindings::cyrup::ext::models::set_model(&m);
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = m;
            Ok(())
        }
    }
    pub fn send_message(&self, message: impl Serialize, opts: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&message).unwrap_or_else(|_| "null".into());
        let o = serde_json::to_string(&opts).unwrap_or_else(|_| "{}".into());
        control(Control::SendMessage(&m, &o))
    }
    pub fn send_user_message(&self, content: &str, opts: impl Serialize) -> Result<(), String> {
        let o = serde_json::to_string(&opts).unwrap_or_else(|_| "{}".into());
        control(Control::SendUserMessage(content, &o))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum Control<'a> {
    NewSession(&'a str),
    Switch(&'a str, &'a str),
    Fork(&'a str, &'a str),
    Navigate(&'a str, &'a str),
    Reload,
    Compact(&'a str),
    WaitIdle,
    SendMessage(&'a str, &'a str),
    SendUserMessage(&'a str, &'a str),
}

fn control(op: Control<'_>) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::guest::bindings::cyrup::ext::control as c;
        return match op {
            Control::NewSession(o) => c::new_session(o),
            Control::Switch(id, o) => c::switch(id, o),
            Control::Fork(id, o) => c::fork(id, o),
            Control::Navigate(id, o) => c::navigate(id, o),
            Control::Reload => c::reload(),
            Control::Compact(o) => c::compact(o),
            Control::WaitIdle => c::wait_idle(),
            Control::SendMessage(m, o) => c::send_message(m, o),
            Control::SendUserMessage(co, o) => c::send_user_message(co, o),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = op;
        Ok(())
    }
}

// --- withSession re-binding (Pi ReplacedSessionContext, types.ts:374-390; sdk gap #3) ---

/// A guest `withSession(ctx)` re-binding closure (Pi types.ts:382).
pub type WithSessionFn = Box<dyn Fn(&ReplacedSessionContext) -> Result<(), String> + 'static>;

thread_local! {
    /// `(next_id, id -> closure)` — the pending `withSession` closures (single-threaded wasm guest).
    static WITH_SESSION: RefCell<(u64, HashMap<String, WithSessionFn>)> =
        RefCell::new((0, HashMap::new()));
}

/// Store a `withSession` closure, returning the id embedded in the `control.*` opts so the host can
/// schedule the matching `with-session` export call after re-binding the session (sdk gap #3).
#[doc(hidden)]
pub fn register_with_session(f: WithSessionFn) -> String {
    WITH_SESSION.with(|c| {
        let mut g = c.borrow_mut();
        g.0 += 1;
        let id = format!("ws-{}", g.0);
        g.1.insert(id.clone(), f);
        id
    })
}

/// Run (and consume) the stored `withSession` closure for `id` against a freshly-bound
/// [`ReplacedSessionContext`] — the host calls this via the `with-session` export after the session
/// is re-bound. An unknown id is a no-op (never an error).
#[doc(hidden)]
pub fn run_with_session(id: &str) -> Result<(), String> {
    let f = WITH_SESSION.with(|c| c.borrow_mut().1.remove(id));
    match f {
        Some(f) => f(&ReplacedSessionContext::new()),
        None => Ok(()),
    }
}

/// Serialize `opts` and inject the registered `withSession` callback id (sdk gap #3).
fn opts_with_callback(opts: impl Serialize, with_session: WithSessionFn) -> String {
    let id = register_with_session(with_session);
    let mut v = serde_json::to_value(&opts).unwrap_or_else(|_| json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("withSessionCallbackId".into(), json!(id));
    }
    v.to_string()
}

/// A fresh command-capable context bound to the replacement session after `newSession`/`fork`/
/// `switchSession` (Pi `ReplacedSessionContext extends ExtensionCommandContext`, types.ts:374-390;
/// sdk gap #3). Passed to the `withSession` closure. Derefs to [`CommandCtx`], so every command-tier
/// op (incl. `send_message`/`send_user_message`) is available on the re-bound session.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReplacedSessionContext {
    cmd: CommandCtx,
}

impl ReplacedSessionContext {
    pub fn new() -> Self {
        Self { cmd: CommandCtx::new() }
    }
    /// The underlying command-tier context bound to the replacement session.
    pub fn command(&self) -> &CommandCtx {
        &self.cmd
    }
}

impl core::ops::Deref for ReplacedSessionContext {
    type Target = CommandCtx;
    fn deref(&self) -> &CommandCtx {
        &self.cmd
    }
}

/// The tool `execute` cancellation signal (Pi `ToolDefinition.execute` `signal: AbortSignal`,
/// types.ts:466; sdk gap #1). A long-running tool polls [`Self::is_aborted`] to cooperatively stop;
/// it reads the host's live cancellation state for this `call_id` (the run `CancelToken`, the epoch
/// deadline, or a named `ui.abort-signal` matching the call id). The host epoch is the hard backstop.
#[derive(Clone, Debug, Default)]
pub struct Signal {
    // Read only by the wasm32 `is_aborted` import call; inert on the host target.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    call_id: String,
}

impl Signal {
    pub fn new(call_id: impl Into<String>) -> Self {
        Self { call_id: call_id.into() }
    }
    /// Whether cancellation has been requested for this tool call (Pi `signal.aborted`).
    pub fn is_aborted(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::host_tool::is_cancelled(&self.call_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }
}

/// The call passed to a guest tool's `execute` (Pi `ToolDefinition.execute` args, types.ts:464).
/// Carries the `toolCallId`, parsed `params`, the cancellation [`Signal`], and a [`Ctx`];
/// `emit_update` streams partial output back to the runtime (Pi `onUpdate`).
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub call_id: String,
    pub params: Value,
    pub ctx: Ctx,
    /// The cancellation signal (Pi `signal`): poll [`Signal::is_aborted`] inside a long `execute`.
    pub signal: Signal,
}

impl ToolCall {
    pub fn new(call_id: impl Into<String>, params: Value) -> Self {
        let call_id = call_id.into();
        Self { signal: Signal::new(call_id.clone()), call_id, params, ctx: Ctx }
    }
    /// The cancellation signal for this call (Pi `signal` param).
    pub fn signal(&self) -> &Signal {
        &self.signal
    }
    /// Stream a partial-output chunk (Pi `onUpdate`).
    pub fn emit_update(&self, chunk: impl Serialize) {
        let chunk_json = serde_json::to_string(&chunk).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::host_tool::emit_update(&self.call_id, &chunk_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = chunk_json;
    }
}
