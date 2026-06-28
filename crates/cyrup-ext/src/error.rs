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
    /// Invalid tool `parameters` JSON-Schema at registration (R-ARCH-EXT-008).
    #[error("invalid tool schema: {0}")]
    Schema(String),
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
