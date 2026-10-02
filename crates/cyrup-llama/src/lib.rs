//! cyrup-llama — llama.cpp router support, ported from pi's bundled llama.cpp extension
//! (`packages/coding-agent/src/extensions/llama/`, cloned at `tmp/pi`; EXT-027).
//!
//! Pi never spawns `llama-server`. This crate, like the upstream extension, is a plain HTTP client
//! of an already-running llama-server in router mode plus a Hugging Face search client. It
//! contributes one provider (`llama.cpp`) and one command (`/llama`) through a single
//! `cyrup_ext::native::NativeExtension`.
//!
//! Modules:
//!   * [`client`]      — the router HTTP client and its SSE reader (`client.ts`)
//!   * [`huggingface`] — Hugging Face search, quantization parsing, token lookup (`huggingface.ts`)
//!   * [`model`]       — router catalog entry → cyrup `Model` mapping (`provider.ts` `toPiModel`)
//!   * [`provider`]    — the `Provider` and its API-key login (`provider.ts`)
//!   * [`ui`]          — the `/llama` manager overlay (`ui.ts`)
//!   * [`extension`]   — the `NativeExtension`, the `/llama` command flow and the constructor
//!     (`index.ts`)
//!
//! Upstream pin: every `@v0.99.2-17` citation in this crate (and in `cyrup-provider`, `cyrup-ext`,
//! `cyrup-config`, `cyrup-session-svc`) means pi commit `70c036211` (`feat(coding-agent): show the
//! durable TUI's task panel by default`), the 17th first-parent commit after `v0.99.2`. The tag
//! `v0.99.2-17` itself does not exist in the `tmp/pi` clone, which sits at `v1.0.0-2-g7fbbd5f4a`;
//! line numbers cited from `resource-loader.ts` are that checkout's.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![forbid(unsafe_code)]

pub mod client;
pub mod extension;
pub mod huggingface;
pub mod model;
pub mod provider;
pub mod ui;

pub use extension::LlamaExtension;

/// The llama.cpp extension as the host loads it, mirroring the other `*_extension_for_env`
/// constructors (`cyrup_mcp::mcp_extension_for_env`, `cyrup_flux::flux_extension_for_env`).
///
/// **This gate does not gate.** Pi's `builtin:llama.cpp` is present in every session of every mode
/// (`extensions/index.ts`: `builtInExtensions = [{ name: "llama.cpp", factory: llamaExtension,
/// builtin: true }]`; the loader hides it, `core/resource-loader.ts:729`), and switching it off is
/// `--no-extensions`' job, which reaches it through
/// `NativeExtension::is_ambient`, not through this function. The `Option` return exists so the
/// call site reads like its siblings' and a future gate has somewhere to live.
///
/// `agent_dir` is the agent directory the binary resolved for every extension; the `/llama`
/// overlay reads its `keybindings.json` on each open.
#[must_use]
pub fn llama_extension_for_env(
    agent_dir: &std::path::Path,
) -> Option<std::sync::Arc<dyn cyrup_ext::native::NativeExtension>> {
    Some(std::sync::Arc::new(LlamaExtension::new(
        agent_dir.to_path_buf(),
    )))
}

/// The provider id pi registers (`provider.ts` `LLAMA_PROVIDER_ID`).
pub const LLAMA_PROVIDER_ID: &str = "llama.cpp";

#[cfg(test)]
mod tests;
