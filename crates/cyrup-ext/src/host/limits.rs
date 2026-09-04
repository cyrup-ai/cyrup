//! Per-instance memory cap via Wasmtime's `ResourceLimiter` (arch-08 §5.3, R-ARCH-EXT-012). A
//! runaway extension cannot exhaust host RSS: a growth past the cap is denied and surfaced as
//! `ExtError::OutOfMemory` (the denial error carries [`OOM_SENTINEL`] so the containment mapper can
//! classify it).

/// Embedded in the denial error so [`crate::host::engine::map_wasm_error`] classifies it as OOM.
pub const OOM_SENTINEL: &str = "cyrup-ext:oom";

/// A `ResourceLimiter` that caps linear-memory and table growth per instance.
#[derive(Clone, Debug)]
pub struct StoreLimits {
    max_memory: usize,
    max_tables: u32,
    max_instances: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        // 64 MiB default per-extension cap (bounded memory, R-00-002).
        Self {
            max_memory: 64 * 1024 * 1024,
            max_tables: 10_000,
            max_instances: 100,
        }
    }
}

impl StoreLimits {
    #[must_use]
    pub fn with_max_memory(mut self, bytes: usize) -> Self {
        self.max_memory = bytes;
        self
    }

    pub fn max_memory(&self) -> usize {
        self.max_memory
    }
}

impl wasmtime::ResourceLimiter for StoreLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.max_memory {
            // Returning Err aborts the growth as a trap, which we map to OutOfMemory.
            Err(wasmtime::Error::msg(format!(
                "{OOM_SENTINEL}: requested {desired} bytes exceeds cap {}",
                self.max_memory
            )))
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired as u32 <= self.max_tables)
    }

    fn instances(&self) -> usize {
        self.max_instances
    }
}
