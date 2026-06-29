//! The Tier-1 build/artifact-cache loop (arch-08 §6.4, R-ARCH-EXT-004/015/016). An agent authors a
//! crate under `.cyrup/extensions/<name>/`; the host content-addresses it, and on a cache miss
//! builds it via `cargo` -> `wasm32-wasip2`. If the wasm/component toolchain is unavailable, the
//! loop + loader are present but the live build/load path is tooling-gated (surfaced, never a crash).

pub mod cache;
pub mod toolchain;

pub use cache::{cache_key, ArtifactCache, CacheKey};
pub use toolchain::{detect_toolchain, Toolchain, ToolchainStatus};

use crate::error::ExtError;
use crate::manifest::HOST_WORLD;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build an authored extension crate to a `cyrup:ext` COMPONENT (arch-08 §6.4; R-08-031 /
/// R-ARCH-EXT-004/015/016). Content-addresses the crate first: a cache HIT (same source ⊕ toolchain
/// ⊕ world) returns the stored component without invoking `cargo`. On a miss it requires the
/// toolchain (surfaced cleanly if absent), runs `cargo build --target wasm32-wasip2` (the
/// wasm32-wasip2 linker componentizes directly), locates the artifact, validates the component
/// preamble, stores it under the cache key, and returns the bytes. A build failure surfaces
/// `ExtError::Build` with the captured diagnostics — never a panic.
pub fn build_component(crate_dir: &Path) -> Result<Vec<u8>, ExtError> {
    build_component_in(crate_dir, &ArtifactCache::default_location())
}

/// As [`build_component`] but with an explicit artifact cache (used by tests).
pub fn build_component_in(crate_dir: &Path, cache: &ArtifactCache) -> Result<Vec<u8>, ExtError> {
    let manifest_path = crate_dir.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(ExtError::Build(format!(
            "no Cargo.toml at extension crate `{}`",
            crate_dir.display()
        )));
    }

    let toolchain = detect_toolchain();
    let key = {
        let src = cache::hash_source_tree(crate_dir)?;
        cache_key(&src, &toolchain.id(), HOST_WORLD)
    };

    // Cache hit: skip cargo entirely (R-ARCH-EXT-016).
    if cache.is_hit(&key) {
        return std::fs::read(cache.artifact_for(&key)).map_err(ExtError::from);
    }

    // Miss: a real build is required. Only `cargo` + the wasm target are needed — the wasm32-wasip2
    // linker componentizes directly, so missing `wasm-tools` does NOT gate the build (gap-08 #6).
    toolchain::require_buildable(&toolchain)?;

    let target = toolchain.target; // "wasm32-wasip2"
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--target")
        .arg(target)
        .output()
        .map_err(|e| ExtError::Build(format!("failed to spawn cargo: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ExtError::Build(format!(
            "cargo build for `{}` failed:\n{}",
            crate_dir.display(),
            stderr.trim()
        )));
    }

    let artifact = locate_artifact(crate_dir, &manifest_path, target)?;
    let bytes = std::fs::read(&artifact).map_err(ExtError::from)?;
    validate_component(&bytes)?;
    cache.store(&key, &bytes)?;
    Ok(bytes)
}

/// Locate the `.wasm` artifact a build produced under `<target-dir>/<triple>/debug/<crate>.wasm`.
fn locate_artifact(crate_dir: &Path, manifest_path: &Path, triple: &str) -> Result<PathBuf, ExtError> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate_dir.join("target"));
    let out_dir = target_dir.join(triple).join("debug");

    // Prefer the crate-named artifact; else fall back to any single `.wasm` in the dir.
    if let Some(name) = package_name(manifest_path) {
        let candidate = out_dir.join(format!("{}.wasm", name.replace('-', "_")));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let mut wasms: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .map_err(ExtError::from)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "wasm").unwrap_or(false))
        .collect();
    wasms.sort();
    wasms.into_iter().next().ok_or_else(|| {
        ExtError::Build(format!("no .wasm artifact found in {}", out_dir.display()))
    })
}

/// Minimal `[package] name = "..."` extraction (no `toml` dep, consistent with the JSON-only host).
fn package_name(manifest_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest_path).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = t.strip_prefix("name")
        {
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            return Some(rest.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

/// Validate the artifact is a wasm COMPONENT (preamble `\0asm` + component layer `0x0a 0x00`),
/// not a bare core module (arch-08 §4.1). Surfaced as a build error, never a load-time trap.
fn validate_component(bytes: &[u8]) -> Result<(), ExtError> {
    // Preamble `00 61 73 6d <version:u16> <layer:u16>`. A core module is `01 00 00 00`
    // (version 1, layer 0); a COMPONENT is `0d 00 01 00` (version 0x0d, layer 1).
    let ok = bytes.get(0..4) == Some(&b"\0asm"[..]) && bytes.get(6..8) == Some(&[0x01, 0x00][..]);
    if ok {
        Ok(())
    } else {
        Err(ExtError::Build(
            "built artifact is not a wasm component (got a core module?)".into(),
        ))
    }
}
