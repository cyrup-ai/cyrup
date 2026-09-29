//! `ExtError` — the extension-host error vocabulary (arch-08 §8). `thiserror` only (libs never
//! use `anyhow`). Every guest fault (trap / OOM / epoch timeout) is mapped to a variant here and
//! surfaced by the dispatcher; the host never crashes (R-00-009 / R-08-036).

/// Extension host error (arch-08 §8).
#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    #[error("cancelled")]
    Cancelled,
    /// A wasm guest trapped (unreachable, integer-divide-by-zero, etc.). Caught, surfaced.
    #[error("extension trapped: {0}")]
    Trap(String),
    /// Guest exceeded its epoch deadline and was preempted (R-ARCH-EXT-012).
    #[error("extension timed out (epoch deadline)")]
    EpochTimeout,
    /// `ResourceLimiter` denied a memory/table growth (R-ARCH-EXT-012).
    #[error("memory limit exceeded")]
    OutOfMemory,
    /// A native handler panicked; contained via catch_unwind (R-08-036).
    #[error("extension panicked: {0}")]
    Panicked(String),
    /// Session-mutation attempted from an event handler (R-08-008).
    #[error("deadlock guard: session-mutation from event handler")]
    Deadlock,
    /// Project not trusted; extension not loaded (R-ARCH-EXT-017).
    #[error("untrusted project: extension not loaded")]
    Untrusted,
    /// wasm32-wasip2 / componentization toolchain unavailable (R-ARCH-EXT-015).
    #[error("toolchain missing: {0}")]
    Toolchain(String),
    /// `cargo build` failed; carries diagnostics (R-ARCH-EXT-016).
    #[error("build failed: {0}")]
    Build(String),
    /// World-version incompatibility recorded in the manifest (arch-08 §4.1).
    #[error("world version mismatch: found {found}, required {required}")]
    WorldVersion { found: String, required: String },
    /// A malformed `capabilities` declaration in `extension.json` (EXT-054). Fails the load rather
    /// than dropping the grant: a typo that quietly widens or narrows the sandbox is precisely the
    /// failure EXT-054 was filed for.
    #[error("invalid capability declaration: {0}")]
    Capability(String),
    /// Invalid tool `parameters` JSON-Schema at registration (R-ARCH-EXT-008).
    #[error("invalid tool schema: {0}")]
    Schema(String),
    /// A registration pi's `ExtensionAPI` refuses by THROWING inside the factory, which fails the
    /// load (`registerTool`'s object-schema guard and `registerFlag`'s typed-default guard,
    /// `core/extensions/loader.ts:274-279` / `:312-316` @v0.87.1). Displayed verbatim: the text is
    /// pi's own `Error` message. EXT-082.
    #[error("{0}")]
    Registration(String),
    /// A duplicate extension id was loaded.
    #[error("duplicate extension id: {0}")]
    DuplicateId(String),
    /// The wasm-host feature is compiled out but a wasm path was requested.
    #[error("wasm host disabled (build with feature \"wasm-host\")")]
    WasmHostDisabled,
    /// Wasmtime engine/linker construction failed.
    #[error("wasm engine init failed: {0}")]
    Engine(String),
    /// Component instantiation/load failed.
    #[error("component load failed: {0}")]
    Component(String),
    /// A `user_bash` emission ABORTED the user's `!`/`!!` (or JSON-RPC `bash`) command — pi's
    /// `throw` out of `emitUserBash` since coding-agent 0.86.0 (*Breaking*, #9068:
    /// "`user_bash` now fails closed: errors or invalid defined results abort the command without
    /// invoking later handlers or executing locally"). Carried out of
    /// `cyrup-session-svc`'s `execute_bash_with_user_event` so that NOTHING runs on the local shell
    /// (EXT-077).
    ///
    /// `Display` is `{0}` and nothing else, because the string is already the user-facing sentence:
    /// either the dispatcher's `Extension failed, blocking execution: …` (pi
    /// `agent-session.ts:475-487`) or [`crate::INVALID_USER_BASH_RESULT_MESSAGE`] verbatim. The
    /// enclosing `SessionServiceError::Extension` adds its own `extension host: ` prefix on the way
    /// out, where pi's RPC `error` field carries the thrown message bare; that prefix is the only
    /// difference from upstream on this path, and it names the layer the refusal came from rather
    /// than changing what happened — the command is aborted and the reason is reported either way.
    #[error("{0}")]
    UserBashAborted(String),
    #[error("io: {0}")]
    Io(String),
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

impl From<std::io::Error> for ExtError {
    fn from(e: std::io::Error) -> Self {
        ExtError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for ExtError {
    fn from(e: serde_json::Error) -> Self {
        ExtError::Core(cyrup_core::CoreError::Serde(e))
    }
}
