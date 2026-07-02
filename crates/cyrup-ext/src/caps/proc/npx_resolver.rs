//! Direct Rust port of `pi-mcp-adapter/npx-resolver.ts`'s `resolveNpxBinary` — resolves an
//! `npx`/`npm exec`-shaped invocation down to the REAL underlying binary the `npx`/`npm` wrapper
//! would itself ultimately `exec`, so [`super::ProcCaps::spawn`] can launch that binary directly
//! instead of `npx`/`npm`.
//!
//! Why this matters (the bug this closes): `npx`/`npm` is itself a full Node "npm-cli" process
//! that spawns the real target package as a CHILD of itself (through several more layers for
//! `npx`, which is a thin `npm exec` wrapper). [`super::ProcCaps::kill`] signals only the ONE pid
//! it tracks (`entry.pid` — see that fn's doc); if that pid is the `npx`/`npm` launcher rather
//! than the real MCP server, the real server survives `kill` as an orphaned grandchild. Real
//! consumer wiring: `pi-mcp-adapter/server-manager.ts:93-104` — `createConnection` calls
//! `resolveNpxBinary(command, args)` and, when it resolves, substitutes `command`/`args` BEFORE
//! constructing `StdioClientTransport` (`:106-112`), i.e. before the real child is ever spawned.
//! That is exactly mirrored here: [`resolve_npx_binary`] is called, and its result substituted,
//! before [`super::ProcCaps::spawn`] ever calls `tokio::process::Command::new`.
//!
//! One deliberate shape divergence: `forceNpxCache` (`npx-resolver.ts:231-250`) is `async` in Pi,
//! awaiting a `child_process.spawn` Promise while Node's event loop serves other work. cyrup's
//! [`super::ProcCaps::spawn`] is a synchronous fn (see its own doc: "no `.await` needed"), so
//! [`force_npx_cache`] below performs the SAME bounded subprocess-spawn-and-wait via blocking
//! `std::process::Command`, not `tokio::process`. The whole resolution (including this) is run by
//! the ONE call site in `spawn` inside `tokio::task::block_in_place`, exactly the bridge pattern
//! `cyrup-session-svc/src/host_services.rs` already uses for other occasionally-slow, inherently-
//! blocking calls (`http_request`, `exec`) — it tells the tokio multi-thread runtime this worker
//! is about to block so other tasks can be moved off it, rather than silently stalling the pool.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// `npx-resolver.ts:7` `CACHE_VERSION`.
const CACHE_VERSION: u32 = 1;
/// `npx-resolver.ts:8` `CACHE_TTL_MS` (24h).
const CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// `npx-resolver.ts:229` `FORCE_CACHE_TIMEOUT_MS`.
const FORCE_CACHE_TIMEOUT: Duration = Duration::from_secs(30);

/// `npx-resolver.ts:22-26` `NpxResolution`.
#[derive(Debug, Clone)]
pub(super) struct NpxResolution {
    pub(super) bin_path: String,
    pub(super) extra_args: Vec<String>,
    pub(super) is_js: bool,
}

/// `npx-resolver.ts:10-15` `NpxCacheEntry`.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
struct NpxCacheEntry {
    #[serde(rename = "resolvedBin")]
    resolved_bin: String,
    #[serde(rename = "resolvedAt")]
    resolved_at: u64,
    #[serde(rename = "packageVersion", skip_serializing_if = "Option::is_none")]
    package_version: Option<String>,
    #[serde(rename = "isJs")]
    is_js: bool,
}

/// `npx-resolver.ts:17-20` `NpxCache`.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
struct NpxCache {
    version: u32,
    entries: HashMap<String, NpxCacheEntry>,
}

/// `npx-resolver.ts:28-32` `ParsedInvocation`.
struct ParsedInvocation {
    package_spec: String,
    bin_name: Option<String>,
    extra_args: Vec<String>,
}

/// `npx-resolver.ts:34-69` `resolveNpxBinary`. Returns `None` wherever the TS returns `null`
/// (unparsable invocation, no npm cache dir, package not found even after a forced cache
/// population attempt) — the caller falls back to running `command`/`args` verbatim, exactly like
/// `server-manager.ts:97-104`'s `if (resolved) { ... }` leaves `command`/`args` untouched when
/// `resolveNpxBinary` resolves to `null`.
pub(super) fn resolve_npx_binary(command: &str, args: &[String]) -> Option<NpxResolution> {
    let parsed = match command {
        "npx" => parse_npx_args(args)?,
        "npm" => parse_npm_exec_args(args)?,
        _ => return None,
    };

    let cache_key = cache_key(command, args);
    if let Some(cached) = load_cache().and_then(|c| c.entries.get(&cache_key).cloned())
        && now_ms().saturating_sub(cached.resolved_at) < CACHE_TTL_MS
        && Path::new(&cached.resolved_bin).exists()
    {
        return Some(NpxResolution {
            bin_path: cached.resolved_bin,
            extra_args: parsed.extra_args,
            is_js: cached.is_js,
        });
    }

    if let Some(resolved) = resolve_from_npm_cache(&parsed.package_spec, parsed.bin_name.as_deref())
    {
        save_cache_entry(&cache_key, &resolved);
        return Some(NpxResolution {
            bin_path: resolved.resolved_bin,
            extra_args: parsed.extra_args,
            is_js: resolved.is_js,
        });
    }

    // Slow path: force npx cache population (npx-resolver.ts:60-61).
    force_npx_cache(&parsed.package_spec);
    let resolved_after_install =
        resolve_from_npm_cache(&parsed.package_spec, parsed.bin_name.as_deref())?;
    save_cache_entry(&cache_key, &resolved_after_install);
    Some(NpxResolution {
        bin_path: resolved_after_install.resolved_bin,
        extra_args: parsed.extra_args,
        is_js: resolved_after_install.is_js,
    })
}

/// `npx-resolver.ts:71-121` `parseNpxArgs`.
fn parse_npx_args(args: &[String]) -> Option<ParsedInvocation> {
    let separator_index = args.iter().position(|a| a == "--");
    let (before, after): (&[String], &[String]) = match separator_index {
        Some(idx) => (args.get(..idx)?, args.get(idx + 1..)?),
        None => (args, &[]),
    };

    let mut positionals: Vec<String> = Vec::new();
    let mut package_spec: Option<String> = None;
    let mut saw_package_flag = false;
    let mut found_first_positional = false;

    let mut i = 0usize;
    while i < before.len() {
        let arg = before.get(i)?;
        if found_first_positional {
            positionals.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "-y" || arg == "--yes" {
            i += 1;
            continue;
        }
        if arg == "-p" || arg == "--package" {
            let value = before.get(i + 1)?;
            if value.starts_with('-') {
                return None;
            }
            if package_spec.is_none() {
                package_spec = Some(value.clone());
            }
            saw_package_flag = true;
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--package=") {
            if value.is_empty() {
                return None;
            }
            if package_spec.is_none() {
                package_spec = Some(value.to_string());
            }
            saw_package_flag = true;
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        positionals.push(arg.clone());
        found_first_positional = true;
        i += 1;
    }

    if saw_package_flag {
        let bin_name = positionals.first()?.clone();
        let package_spec = package_spec?;
        let mut extra_args = positionals.get(1..).unwrap_or(&[]).to_vec();
        extra_args.extend(after.iter().cloned());
        return Some(ParsedInvocation { package_spec, bin_name: Some(bin_name), extra_args });
    }

    let package_positional = positionals.first()?.clone();
    let mut extra_args = positionals.get(1..).unwrap_or(&[]).to_vec();
    extra_args.extend(after.iter().cloned());
    Some(ParsedInvocation { package_spec: package_positional, bin_name: None, extra_args })
}

/// `npx-resolver.ts:123-158` `parseNpmExecArgs`.
fn parse_npm_exec_args(args: &[String]) -> Option<ParsedInvocation> {
    if args.first().map(String::as_str) != Some("exec") {
        return None;
    }
    let exec_args = args.get(1..)?;
    let separator_index = exec_args.iter().position(|a| a == "--")?;
    let before = exec_args.get(..separator_index)?;
    let after = exec_args.get(separator_index + 1..)?;

    let mut package_spec: Option<String> = None;
    let mut i = 0usize;
    while i < before.len() {
        let arg = before.get(i)?;
        if arg == "-y" || arg == "--yes" {
            i += 1;
            continue;
        }
        if arg == "--package" {
            let value = before.get(i + 1)?;
            if value.starts_with('-') {
                return None;
            }
            if package_spec.is_none() {
                package_spec = Some(value.clone());
            }
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--package=") {
            if value.is_empty() {
                return None;
            }
            if package_spec.is_none() {
                package_spec = Some(value.to_string());
            }
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        i += 1;
    }

    let bin_name = after.first()?.clone();
    let package_spec = package_spec?;
    let extra_args = after.get(1..).unwrap_or(&[]).to_vec();
    Some(ParsedInvocation { package_spec, bin_name: Some(bin_name), extra_args })
}

/// `npx-resolver.ts:160-227` `resolveFromNpmCache`. Thin wrapper over
/// [`resolve_from_npm_cache_at`] that supplies the REAL host npm cache dir
/// ([`get_npm_cache_dir`]) — split out so tests can inject a hermetic fake cache dir instead
/// (`cyrup-ext` is `#![forbid(unsafe_code)]` crate-wide and edition 2024 makes
/// `std::env::set_var` `unsafe fn`, so tests cannot point `NPM_CONFIG_CACHE` at a fixture dir the
/// way a real npm invocation would — same reasoning [`interpolate_env_vars_with`]'s doc, `proc.rs`,
/// already documents for the sibling `${VAR}` interpolation tests).
fn resolve_from_npm_cache(package_spec: &str, bin_name: Option<&str>) -> Option<NpxCacheEntry> {
    resolve_from_npm_cache_at(&get_npm_cache_dir()?, package_spec, bin_name)
}

fn resolve_from_npm_cache_at(
    cache_dir: &Path,
    package_spec: &str,
    bin_name: Option<&str>,
) -> Option<NpxCacheEntry> {
    let package_name = extract_package_name(package_spec)?;
    let package_dir = find_cached_package_dir(cache_dir, &package_name)?;

    let package_json_path = package_dir.join("package.json");
    if !package_json_path.is_file() {
        return None;
    }
    let raw = fs::read_to_string(&package_json_path).ok()?;
    let pkg: PackageJson = serde_json::from_str(&raw).ok()?;
    let bin_field = pkg.bin?;

    let candidates = build_bin_candidates(&package_name, bin_name);
    let mut chosen_bin_name: Option<String> = None;
    let mut bin_rel: Option<String> = None;

    match &bin_field {
        BinField::Single(s) => {
            chosen_bin_name = Some(default_bin_name(&package_name));
            bin_rel = Some(s.clone());
        }
        BinField::Map(map) => {
            for candidate in &candidates {
                if let Some(rel) = map.get(candidate) {
                    chosen_bin_name = Some(candidate.clone());
                    bin_rel = Some(rel.clone());
                    break;
                }
            }
            if bin_rel.is_none() {
                // npx-resolver.ts:201-207 falls back to `Object.entries(binField)[0]` — JS object
                // KEY-INSERTION order. `serde_json` here has no `preserve_order` feature enabled
                // workspace-wide (Cargo.toml has no such feature turned on anywhere in-tree), so
                // this map iterates in an ARBITRARY (HashMap) order instead. Only reachable when a
                // `package.json`'s `bin` map has ZERO keys matching any derived candidate name —
                // an unusual, malformed-ish shape for a real npm package — and even then this still
                // returns a genuinely valid, existing bin path from the SAME package, just possibly
                // a different one than Node would have picked. Deliberately accepted rather than
                // adding an order-preserving JSON dependency for this one rare edge.
                if let Some((k, v)) = map.iter().next() {
                    chosen_bin_name = Some(k.clone());
                    bin_rel = Some(v.clone());
                }
            }
        }
    }

    let bin_rel = bin_rel?;

    let node_modules_dir = find_node_modules_dir(&package_dir);
    let bin_link = chosen_bin_name.as_ref().map(|n| node_modules_dir.join(".bin").join(n));
    let mut resolved_bin =
        bin_link.filter(|p| p.exists()).and_then(|p| fs::canonicalize(&p).ok());
    if resolved_bin.is_none() {
        let candidate = package_dir.join(&bin_rel);
        if !candidate.exists() {
            return None;
        }
        resolved_bin = Some(candidate);
    }
    let resolved_bin = resolved_bin?;

    let is_js = detect_js_binary(&resolved_bin);
    Some(NpxCacheEntry {
        resolved_bin: resolved_bin.to_string_lossy().into_owned(),
        resolved_at: now_ms(),
        package_version: pkg.version,
        is_js,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct PackageJson {
    bin: Option<BinField>,
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BinField {
    Single(String),
    Map(HashMap<String, String>),
}

/// `npx-resolver.ts:231-250` `forceNpxCache` — see the module doc for why this blocks via
/// `std::process::Command` rather than `tokio::process`. Every failure mode (spawn error, timeout,
/// non-zero exit) is swallowed exactly like the TS `catch { /* Ignore failures ... */ }`: the
/// caller's subsequent [`resolve_from_npm_cache`] retry simply comes up empty and
/// [`resolve_npx_binary`] returns `None`, falling back to running `npx`/`npm` unresolved.
fn force_npx_cache(package_spec: &str) {
    let spawned = Command::new("npm")
        .args(["exec", "--yes", "--package", package_spec, "--", "node", "-e", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else { return };

    let deadline = Instant::now() + FORCE_CACHE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return,
        }
    }
}

/// `npx-resolver.ts:252-266` `buildBinCandidates`.
fn build_bin_candidates(package_name: &str, explicit_bin: Option<&str>) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(b) = explicit_bin {
        candidates.push(b.to_string());
    }

    if package_name.starts_with('@') {
        let parts: Vec<&str> = package_name.split('/').collect();
        let name_part = parts.get(1).copied().unwrap_or("");
        let scope_part = parts.first().copied().unwrap_or("").replace('@', "");
        if !name_part.is_empty() {
            candidates.push(name_part.to_string());
        }
        if !scope_part.is_empty() && !name_part.is_empty() {
            candidates.push(format!("{scope_part}-{name_part}"));
        }
    } else {
        candidates.push(package_name.to_string());
    }

    let mut seen = std::collections::HashSet::new();
    candidates.into_iter().filter(|c| !c.is_empty() && seen.insert(c.clone())).collect()
}

/// `npx-resolver.ts:268-282` `extractPackageName`.
fn extract_package_name(spec: &str) -> Option<String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('@') {
        let slash_index = trimmed.find('/')?;
        let at_index = trimmed.rfind('@').unwrap_or(0);
        if at_index > slash_index {
            return trimmed.get(..at_index).map(str::to_string);
        }
        return Some(trimmed.to_string());
    }
    match trimmed.find('@') {
        Some(at_index) => trimmed.get(..at_index).map(str::to_string),
        None => Some(trimmed.to_string()),
    }
}

/// `npx-resolver.ts:284-290` `defaultBinName`.
fn default_bin_name(package_name: &str) -> String {
    if package_name.starts_with('@') {
        let parts: Vec<&str> = package_name.split('/').collect();
        match parts.get(1) {
            Some(p) => (*p).to_string(),
            None => package_name.replace('@', "").replace('/', "-"),
        }
    } else {
        package_name.to_string()
    }
}

/// `npx-resolver.ts:292-317` `findCachedPackageDir`.
fn find_cached_package_dir(cache_dir: &Path, package_name: &str) -> Option<PathBuf> {
    let npx_dir = cache_dir.join("_npx");
    if !npx_dir.is_dir() {
        return None;
    }

    let package_path_parts: Vec<&str> =
        if package_name.starts_with('@') { package_name.split('/').collect() } else { vec![package_name] };

    let mut candidates: Vec<(PathBuf, SystemTime)> = fs::read_dir(&npx_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| {
            let full = e.path();
            let mtime = full.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
            (full, mtime)
        })
        .collect();
    candidates.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    for (dir, _) in candidates {
        let mut pkg_dir = dir.join("node_modules");
        for part in &package_path_parts {
            pkg_dir = pkg_dir.join(part);
        }
        if pkg_dir.join("package.json").is_file() {
            return Some(pkg_dir);
        }
    }
    None
}

/// `npx-resolver.ts:319-326` `findNodeModulesDir`.
fn find_node_modules_dir(package_dir: &Path) -> PathBuf {
    let comps: Vec<std::path::Component<'_>> = package_dir.components().collect();
    if let Some(idx) = comps.iter().rposition(|c| c.as_os_str() == "node_modules") {
        let mut p = PathBuf::new();
        for c in comps.get(..=idx).unwrap_or(&[]) {
            p.push(c.as_os_str());
        }
        return p;
    }
    package_dir.join("..")
}

/// `npx-resolver.ts:328-344` `detectJsBinary`.
fn detect_js_binary(bin_path: &Path) -> bool {
    if let Some(ext) = bin_path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if ext_lower == "js" || ext_lower == "mjs" || ext_lower == "cjs" {
            return true;
        }
    }
    let Ok(mut file) = fs::File::open(bin_path) else { return false };
    let mut buf = [0u8; 256];
    let Ok(n) = file.read(&mut buf) else { return false };
    let text = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[]));
    let first_line = text.split('\n').next().unwrap_or("");
    first_line.starts_with("#!") && first_line.contains("node")
}

/// `npx-resolver.ts:346-367` `getNpmCacheDir`, memoized like the TS module-level
/// `npmCacheDirCached` (module state persists for the life of the process; a `OnceLock` mirrors
/// that for the life of the host process).
fn get_npm_cache_dir() -> Option<PathBuf> {
    static NPM_CACHE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    NPM_CACHE_DIR
        .get_or_init(|| {
            if let Ok(configured) = std::env::var("NPM_CONFIG_CACHE")
                && !configured.is_empty()
            {
                return Some(PathBuf::from(configured));
            }
            let output = Command::new("npm").args(["config", "get", "cache"]).output().ok()?;
            if !output.status.success() {
                return None;
            }
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path.is_empty() { None } else { Some(PathBuf::from(path)) }
        })
        .clone()
}

/// The on-disk resolution cache path. `npx-resolver.ts:369-371` `getNpxCachePath` ->
/// `getAgentPath("mcp-npx-cache.json")` -> `agent-dir.ts`'s `getAgentDir()`. Ported directly
/// (rather than depending on `cyrup-config::Paths`, which needs full CLI+env layering to
/// construct) using the SAME dual env-var convention `cyrup-config/src/env.rs:68` already
/// establishes workspace-wide (`CYRUP_AGENT_DIR`, falling back to the Pi-compatible
/// `PI_CODING_AGENT_DIR`), defaulting to `~/.cyrup/agent` (`cyrup-config/src/env.rs:143`).
fn agent_dir() -> PathBuf {
    let configured = std::env::var("CYRUP_AGENT_DIR")
        .ok()
        .or_else(|| std::env::var("PI_CODING_AGENT_DIR").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(configured) = configured else {
        return super::host_home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".cyrup").join("agent");
    };
    if configured == "~" {
        return super::host_home_dir().unwrap_or_else(|| PathBuf::from(configured));
    }
    if let Some(rest) = configured.strip_prefix("~/").or_else(|| configured.strip_prefix("~\\"))
        && let Some(home) = super::host_home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(configured)
}

fn npx_cache_path() -> PathBuf {
    agent_dir().join("mcp-npx-cache.json")
}

/// `npx-resolver.ts:373-385` `loadCache`.
fn load_cache() -> Option<NpxCache> {
    let raw = fs::read_to_string(npx_cache_path()).ok()?;
    let cache: NpxCache = serde_json::from_str(&raw).ok()?;
    if cache.version != CACHE_VERSION {
        return None;
    }
    Some(cache)
}

/// `npx-resolver.ts:387-408` `saveCacheEntry` — same read-merge-atomic-rename shape (a fresh read
/// immediately before the merge, rather than the loaded `cache` from [`resolve_npx_binary`]'s own
/// earlier call, exactly like the TS re-reading `cachePath` here rather than reusing its own
/// earlier `loadCache()` result).
fn save_cache_entry(key: &str, entry: &NpxCacheEntry) {
    let path = npx_cache_path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }

    let mut merged = load_cache().unwrap_or_else(|| NpxCache { version: CACHE_VERSION, entries: HashMap::new() });
    merged.entries.insert(key.to_string(), entry.clone());

    let Ok(serialized) = serde_json::to_string_pretty(&merged) else { return };
    let tmp_path = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    if fs::write(&tmp_path, serialized).is_err() {
        return;
    }
    let _ = fs::rename(&tmp_path, &path);
}

fn cache_key(command: &str, args: &[String]) -> String {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push(command.to_string());
    all.extend(args.iter().cloned());
    serde_json::to_string(&all).unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_npx_args_plain_package() {
        let parsed = parse_npx_args(&args(&["-y", "@foo/bar"])).expect("parses");
        assert_eq!(parsed.package_spec, "@foo/bar");
        assert_eq!(parsed.bin_name, None);
        assert!(parsed.extra_args.is_empty());
    }

    #[test]
    fn parse_npx_args_with_extra_args_and_separator() {
        let parsed =
            parse_npx_args(&args(&["-y", "@foo/bar", "serve", "--", "--port", "3000"])).expect("parses");
        assert_eq!(parsed.package_spec, "@foo/bar");
        assert_eq!(parsed.extra_args, vec!["serve", "--port", "3000"]);
    }

    #[test]
    fn parse_npx_args_explicit_package_flag() {
        let parsed = parse_npx_args(&args(&["-p", "@foo/bar", "mybin", "x"])).expect("parses");
        assert_eq!(parsed.package_spec, "@foo/bar");
        assert_eq!(parsed.bin_name.as_deref(), Some("mybin"));
        assert_eq!(parsed.extra_args, vec!["x"]);
    }

    #[test]
    fn parse_npx_args_rejects_unknown_leading_flag() {
        assert!(parse_npx_args(&args(&["--unknown", "@foo/bar"])).is_none());
    }

    #[test]
    fn parse_npx_args_empty_is_none() {
        assert!(parse_npx_args(&args(&[])).is_none());
    }

    #[test]
    fn parse_npm_exec_args_requires_separator() {
        assert!(parse_npm_exec_args(&args(&["exec", "--package", "@foo/bar", "mybin"])).is_none());
    }

    #[test]
    fn parse_npm_exec_args_ok() {
        let parsed =
            parse_npm_exec_args(&args(&["exec", "--package", "@foo/bar", "--", "mybin", "x"])).expect("parses");
        assert_eq!(parsed.package_spec, "@foo/bar");
        assert_eq!(parsed.bin_name.as_deref(), Some("mybin"));
        assert_eq!(parsed.extra_args, vec!["x"]);
    }

    #[test]
    fn parse_npm_exec_args_requires_exec_first() {
        assert!(parse_npm_exec_args(&args(&["install", "@foo/bar"])).is_none());
    }

    #[test]
    fn extract_package_name_scoped_with_version() {
        assert_eq!(extract_package_name("@foo/bar@1.2.3").as_deref(), Some("@foo/bar"));
    }

    #[test]
    fn extract_package_name_scoped_no_version() {
        assert_eq!(extract_package_name("@foo/bar").as_deref(), Some("@foo/bar"));
    }

    #[test]
    fn extract_package_name_unscoped_with_version() {
        assert_eq!(extract_package_name("bar@1.2.3").as_deref(), Some("bar"));
    }

    #[test]
    fn extract_package_name_scoped_missing_slash_is_none() {
        assert!(extract_package_name("@foo").is_none());
    }

    #[test]
    fn build_bin_candidates_scoped() {
        let c = build_bin_candidates("@foo/bar", None);
        assert_eq!(c, vec!["bar".to_string(), "foo-bar".to_string()]);
    }

    #[test]
    fn build_bin_candidates_unscoped() {
        let c = build_bin_candidates("bar", None);
        assert_eq!(c, vec!["bar".to_string()]);
    }

    #[test]
    fn default_bin_name_scoped() {
        assert_eq!(default_bin_name("@foo/bar"), "bar");
    }

    #[test]
    fn default_bin_name_unscoped() {
        assert_eq!(default_bin_name("bar"), "bar");
    }

    #[test]
    fn non_npx_command_resolves_to_none() {
        assert!(resolve_npx_binary("node", &args(&["server.js"])).is_none());
    }

    #[test]
    fn unparsable_npx_invocation_resolves_to_none() {
        assert!(resolve_npx_binary("npx", &args(&["--bogus-flag"])).is_none());
    }

    /// End-to-end proof against a REAL filesystem layout mimicking npm's `_npx` cache — no real
    /// `npm`/network involved, so this exercises [`resolve_from_npm_cache`]'s actual directory
    /// walk, `package.json` `bin` map, and `.bin` symlink handling live rather than mocking them.
    #[test]
    fn resolve_from_npm_cache_finds_bin_via_dot_bin_symlink() {
        let tmp = std::env::temp_dir().join(format!(
            "cyrup-npx-resolver-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let hash_dir = tmp.join("_npx").join("abc123");
        let pkg_dir = hash_dir.join("node_modules").join("cowsay");
        fs::create_dir_all(&pkg_dir).expect("mkdir pkg_dir");
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"cowsay","version":"1.0.0","bin":{"cowsay":"cli.js"}}"#,
        )
        .expect("write package.json");
        fs::write(pkg_dir.join("cli.js"), "#!/usr/bin/env node\nconsole.log('moo');\n")
            .expect("write cli.js");
        let bin_dir = hash_dir.join("node_modules").join(".bin");
        fs::create_dir_all(&bin_dir).expect("mkdir .bin");
        #[cfg(unix)]
        std::os::unix::fs::symlink(pkg_dir.join("cli.js"), bin_dir.join("cowsay"))
            .expect("symlink .bin/cowsay");

        #[cfg(unix)]
        {
            let entry = resolve_from_npm_cache_at(&tmp, "cowsay", None).expect("resolves");
            assert!(entry.resolved_bin.ends_with("cli.js"));
            assert!(entry.is_js);
            assert_eq!(entry.package_version.as_deref(), Some("1.0.0"));
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_js_binary_by_extension() {
        let tmp = std::env::temp_dir()
            .join(format!("cyrup-npx-detect-{}-{}.mjs", std::process::id(), now_ms()));
        fs::write(&tmp, "export {}\n").expect("write");
        assert!(detect_js_binary(&tmp));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn detect_js_binary_by_shebang() {
        let tmp = std::env::temp_dir()
            .join(format!("cyrup-npx-detect-shebang-{}-{}", std::process::id(), now_ms()));
        fs::write(&tmp, "#!/usr/bin/env node\nconsole.log(1)\n").expect("write");
        assert!(detect_js_binary(&tmp));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn detect_js_binary_non_js_shebang_is_false() {
        let tmp = std::env::temp_dir()
            .join(format!("cyrup-npx-detect-sh-{}-{}", std::process::id(), now_ms()));
        fs::write(&tmp, "#!/bin/sh\necho hi\n").expect("write");
        assert!(!detect_js_binary(&tmp));
        let _ = fs::remove_file(&tmp);
    }
}
