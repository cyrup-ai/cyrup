//! The Tier-1 build/artifact-cache loop (arch-08 §6.4, R-ARCH-EXT-004/015/016). An agent authors a
//! crate under `.cyrup/extensions/<name>/`; the host content-addresses it, and on a cache miss
//! builds it via `cargo` -> `wasm32-wasip2`. If the wasm/component toolchain is unavailable, the
//! loop + loader are present but the live build/load path is tooling-gated (surfaced, never a crash).

pub mod cache;
pub mod toolchain;

pub use cache::{cache_key, ArtifactCache, CacheKey};
pub use toolchain::{detect_toolchain, Toolchain, ToolchainStatus};
