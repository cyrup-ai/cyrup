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

/// `npx-resolver.ts:10` `EXACT_PACKAGE_VERSION_RE` — MCP-105.
///
/// ```text
/// /^\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?(?:\+[0-9A-Za-z][0-9A-Za-z.-]*)?$/
/// ```
///
/// Two deliberate spelling changes, both because the two engines disagree about what the *source*
/// means rather than about what the pattern should match:
///
/// * `\d` is spelled `[0-9]`. JavaScript's `\d` is ASCII-only; Rust's `regex` makes it
///   Unicode-aware, so `\d+` there would accept `١.٢.٣` and pin a version npm can never produce.
///   The same substitution is made — for `\w`, and for the same reason — in
///   `cyrup_mcp::credentials::interpolate_env_vars`.
/// * `^`/`$` are spelled `\A`/`\z`, which is explicitness rather than a fix: Rust's `$` without
///   `multi_line` already anchors to the end of the haystack, exactly as JavaScript's does without
///   `m`. MEASURED, because the obvious worry turns out to be moot in a more interesting way:
///   `parsePackageSpec("pkg@1.2.3\n")` pins to `"1.2.3"` upstream — `spec.trim()` removes the
///   newline before the pattern runs — so only an INTERIOR newline ever reaches these anchors.
static EXACT_PACKAGE_VERSION: OnceLock<regex::Regex> = OnceLock::new();

fn exact_package_version_re() -> &'static regex::Regex {
    #[allow(clippy::expect_used)]
    EXACT_PACKAGE_VERSION.get_or_init(|| {
        regex::Regex::new(
            r"\A[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?(?:\+[0-9A-Za-z][0-9A-Za-z.-]*)?\z",
        )
        .expect("EXACT_PACKAGE_VERSION_RE is a literal and compiles")
    })
}

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

    // `const packageSpec = parsePackageSpec(parsed.packageSpec);` — computed BEFORE the cache is
    // consulted, because its `exactVersion` is part of the hit predicate (MCP-105).
    let package_spec = parse_package_spec(&parsed.package_spec);
    let cache_key = cache_key(command, args);
    if let Some(cached) = load_cache().and_then(|c| c.entries.get(&cache_key).cloned())
        && cache_entry_is_usable(
            &cached,
            package_spec.as_ref().and_then(|spec| spec.exact_version.as_deref()),
            now_ms(),
        )
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
            // `npx-resolver.ts:88`: `if (!value || value.startsWith("-")) return null;` — JS's
            // `!value` rejects BOTH the out-of-bounds case (`before[i+1]` is `undefined`) AND an
            // empty string. `before.get(i + 1)?` already covers out-of-bounds; the `is_empty()`
            // check below is what closes the empty-string half.
            let value = before.get(i + 1)?;
            if value.is_empty() || value.starts_with('-') {
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
            // `npx-resolver.ts:132`: `if (!value || value.startsWith("-")) return null;` — same
            // empty-string gap as `parse_npx_args` above; see that comment for the JS semantics.
            let value = before.get(i + 1)?;
            if value.is_empty() || value.starts_with('-') {
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
/// way a real npm invocation would — same reasoning [`super::interpolate_env_vars_with`]'s doc, `proc.rs`,
/// already documents for the sibling `${VAR}` interpolation tests).
fn resolve_from_npm_cache(package_spec: &str, bin_name: Option<&str>) -> Option<NpxCacheEntry> {
    resolve_from_npm_cache_at(&get_npm_cache_dir()?, package_spec, bin_name)
}

fn resolve_from_npm_cache_at(
    cache_dir: &Path,
    package_spec: &str,
    bin_name: Option<&str>,
) -> Option<NpxCacheEntry> {
    // `const parsedSpec = parsePackageSpec(packageSpec); if (!parsedSpec) return null;`
    let parsed = parse_package_spec(package_spec)?;
    let package_name = parsed.package_name;
    let package_dir =
        find_cached_package_dir(cache_dir, &package_name, parsed.exact_version.as_deref())?;

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

/// `npx-resolver.ts:60-66` — the cache-hit predicate, as a pure function so it can be asserted
/// without a filesystem or a clock (MCP-105).
///
/// ```text
/// cached
/// && Date.now() - cached.resolvedAt < CACHE_TTL_MS
/// && existsSync(cached.resolvedBin)
/// && (!packageSpec?.exactVersion || cached.packageVersion === packageSpec.exactVersion)
/// ```
///
/// The fourth clause is MCP-105's, and without it a cache entry recorded for `pkg@2.0.0` satisfies
/// a later `pkg@1.0.0` for a whole day — the entry keys on `[command, packageSpec, binName]`, so
/// the two DO occupy different slots, but an entry written before a `package.json` changed under it
/// does not. `cached.packageVersion` absent (`None`) never equals a requested exact version, which
/// is `undefined === "1.0.0"` upstream: also false.
fn cache_entry_is_usable(cached: &NpxCacheEntry, exact_version: Option<&str>, now: u64) -> bool {
    if now.saturating_sub(cached.resolved_at) >= CACHE_TTL_MS {
        return false;
    }
    if !Path::new(&cached.resolved_bin).exists() {
        return false;
    }
    match exact_version {
        None => true,
        Some(wanted) => cached.package_version.as_deref() == Some(wanted),
    }
}

/// `npx-resolver.ts:37-40` `ParsedPackageSpec` — MCP-105.
///
/// Upstream replaced `extractPackageName` with this: the name alone was never enough, because a
/// spec that names an EXACT version has to pin, and one that names a range must not. This port had
/// only the name half, so `npx -y pkg@1.0.0` happily resolved to whichever `_npx` copy had the
/// newest mtime — including `pkg@2.0.0`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedPackageSpec {
    package_name: String,
    /// `Some` only when the requested version is a full semver after normalisation. `^1.2.0`,
    /// `~1.2.0`, `1.2`, `latest` and `""` all carry `None` and pin nothing.
    exact_version: Option<String>,
}

/// `npx-resolver.ts:304-338` `parsePackageSpec` — MCP-105, replacing `extractPackageName`.
///
/// Every arm below was MEASURED against upstream's own function on node 22 (`v2.26.1`, `fafae21`)
/// over 29 specs; the table is reproduced in `parse_package_spec_matches_upstream_case_for_case`.
/// The three that a reading gets wrong:
///
/// * `pkg@01.2.3` DOES pin, to the literal `"01.2.3"` — the pattern is `\d+`, not a semver
///   validator, and npm's own `version` field would have to match that string byte for byte.
/// * `"  spaced@1.0.0  "` trims first, so the name is `spaced`.
/// * `pkg@=v1.2.3` normalises to `1.2.3`: one leading `=` then one leading `v`/`V`, in that order.
///   `pkg@vv1.2.3` therefore pins nothing.
fn parse_package_spec(spec: &str) -> Option<ParsedPackageSpec> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (package_name, requested_version): (&str, Option<&str>) = if trimmed.starts_with('@') {
        // `const slashIndex = trimmed.indexOf("/"); if (slashIndex < 0) return null;`
        let slash_index = trimmed.find('/')?;
        // `const atIndex = trimmed.lastIndexOf("@");` — always `>= 0` here, since index 0 is `@`.
        let at_index = trimmed.rfind('@').unwrap_or(0);
        if at_index > slash_index {
            (trimmed.get(..at_index)?, trimmed.get(at_index + 1..))
        } else {
            (trimmed, None)
        }
    } else {
        match trimmed.find('@') {
            Some(at_index) => (trimmed.get(..at_index)?, trimmed.get(at_index + 1..)),
            None => (trimmed, None),
        }
    };

    // `if (!packageName) return null;`
    if package_name.is_empty() {
        return None;
    }

    // `requestedVersion?.replace(/^=/, "").replace(/^v/i, "")` — ONE `=`, then ONE `v` or `V`.
    let normalized = requested_version.map(|version| {
        let version = version.strip_prefix('=').unwrap_or(version);
        version
            .strip_prefix('v')
            .or_else(|| version.strip_prefix('V'))
            .unwrap_or(version)
    });

    Some(ParsedPackageSpec {
        package_name: package_name.to_string(),
        // `...(normalizedVersion && RE.test(normalizedVersion) ? { exactVersion } : {})` — the
        // `&&` is a truthiness test, so an empty string carries nothing.
        exact_version: normalized
            .filter(|version| !version.is_empty() && exact_package_version_re().is_match(version))
            .map(str::to_string),
    })
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

/// `npx-resolver.ts:348-381` `findCachedPackageDir` — MCP-105 added `exactVersion`.
///
/// The directory list is ordered newest-mtime-first and the FIRST candidate holding the package
/// wins. With `exact_version` set, a candidate whose `package.json` `version` is not that string is
/// skipped instead — which is the whole of the pin: `npx -y pkg@1.0.0` must resolve to the 1.0.0
/// copy even when a 2.0.0 copy was installed more recently. A `package.json` that cannot be read or
/// parsed is skipped too (upstream's `catch { continue }`), NOT treated as a match.
fn find_cached_package_dir(
    cache_dir: &Path,
    package_name: &str,
    exact_version: Option<&str>,
) -> Option<PathBuf> {
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
        let package_json = pkg_dir.join("package.json");
        if !package_json.is_file() {
            continue;
        }
        if let Some(wanted) = exact_version {
            // `const pkg = JSON.parse(readFileSync(...)); if (pkg.version !== exactVersion) continue;`
            let Some(version) = fs::read_to_string(&package_json)
                .ok()
                .and_then(|raw| serde_json::from_str::<PackageJson>(&raw).ok())
                .and_then(|pkg| pkg.version)
            else {
                continue;
            };
            if version != wanted {
                continue;
            }
        }
        return Some(pkg_dir);
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
/// `getAgentPath("mcp-npx-cache.json")` -> `agent-dir.ts`'s `getAgentDir()`.
///
/// Resolved through [`cyrup_config::paths::cyrup_agent_dir_from`], the workspace's one agent-dir
/// ladder. This was a hand-rolled port reading `CYRUP_AGENT_DIR` -> `PI_CODING_AGENT_DIR`, missing
/// `CYRUP_CODING_AGENT_DIR` — the spelling `cyrup-intercom` and `cyrup-ext-subagents` use, which
/// `cyrup-config` has read since CFG-076. Setting it therefore moved the binary's layout and left
/// this cache behind in the old tree. That is MCP-139 gap 1, and sharing the ladder is its fix.
fn agent_dir() -> PathBuf {
    let home = super::host_home_dir().unwrap_or_else(|| PathBuf::from("."));
    cyrup_config::paths::cyrup_agent_dir_from(&home, &|key| std::env::var_os(key))
}

fn npx_cache_path() -> PathBuf {
    agent_dir().join("mcp-npx-cache.json")
}

/// `npx-resolver.ts:373-385` `loadCache`.
fn load_cache() -> Option<NpxCache> {
    load_cache_at(&npx_cache_path())
}

/// Thin wrapper split out (same reasoning as [`resolve_from_npm_cache`]/[`resolve_from_npm_cache_at`]
/// just above) so tests can point it at a hermetic fixture path instead of the real `npx_cache_path()`.
fn load_cache_at(path: &Path) -> Option<NpxCache> {
    let raw = fs::read_to_string(path).ok()?;
    let cache: NpxCache = serde_json::from_str(&raw).ok()?;
    if cache.version != CACHE_VERSION {
        return None;
    }
    Some(cache)
}

/// Serializes [`save_cache_entry`]'s read-merge-write-rename cycle across concurrent
/// [`resolve_npx_binary`] calls within this process.
///
/// Pi's `saveCacheEntry` (`npx-resolver.ts:387-408`) is fully synchronous
/// (`readFileSync`/`writeFileSync`/`renameSync`, no `await`), so Node's single-threaded event loop
/// already guarantees no other JS — including a concurrent `saveCacheEntry` call from a different
/// in-flight `Promise` (e.g. two MCP servers connecting around the same time,
/// `server-manager.ts:73`) — can interleave mid-function, even though every call builds the SAME
/// `${cachePath}.${process.pid}.tmp` tmp-file name (`npx-resolver.ts:405`, same `process.pid`
/// every time). [`resolve_npx_binary`] runs on separate OS threads via
/// `tokio::task::block_in_place` (see this module's top-level doc comment), and [`super::ProcCaps::
/// spawn`] takes `&self` (not `&mut self`), so — without an explicit lock — two genuinely
/// concurrent cold-cache resolutions racing on the identical tmp path could interleave a
/// write/rename (a guest managing multiple npx-backed subprocesses concurrently is a realistic
/// trigger). A process-wide `Mutex` restores Node's single-writer-at-a-time guarantee.
static SAVE_CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `npx-resolver.ts:387-408` `saveCacheEntry` — same read-merge-atomic-rename shape (a fresh read
/// immediately before the merge, rather than the loaded `cache` from [`resolve_npx_binary`]'s own
/// earlier call, exactly like the TS re-reading `cachePath` here rather than reusing its own
/// earlier `loadCache()` result).
fn save_cache_entry(key: &str, entry: &NpxCacheEntry) {
    save_cache_entry_at(&npx_cache_path(), key, entry);
}

/// Thin wrapper split out (same reasoning as [`resolve_from_npm_cache`]/[`resolve_from_npm_cache_at`]
/// above) so tests can drive the real read-merge-write-rename-under-lock cycle against a hermetic
/// fixture path, including concurrently from multiple threads, instead of the real `npx_cache_path()`.
fn save_cache_entry_at(path: &Path, key: &str, entry: &NpxCacheEntry) {
    // A poisoned lock (only reachable if a prior holder panicked mid-cycle, which nothing in this
    // body does) degrades to skipping this save — the SAME graceful "just don't persist" fallback
    // already used for every I/O failure below, never a panic of our own.
    let Ok(_guard) = SAVE_CACHE_LOCK.lock() else { return };

    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }

    let mut merged =
        load_cache_at(path).unwrap_or_else(|| NpxCache { version: CACHE_VERSION, entries: HashMap::new() });
    merged.entries.insert(key.to_string(), entry.clone());

    let Ok(serialized) = serde_json::to_string_pretty(&merged) else { return };
    let tmp_path = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    if fs::write(&tmp_path, serialized).is_err() {
        return;
    }
    let _ = fs::rename(&tmp_path, path);
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

    // npx-resolver.ts:88: `if (!value || value.startsWith("-")) return null;` — JS's `!value`
    // rejects an empty string just as much as a missing/dash-prefixed one. An empty `-p`/`--package`
    // value must not slip through and reach `force_npx_cache("")`.
    #[test]
    fn parse_npx_args_rejects_empty_package_flag_value() {
        assert!(parse_npx_args(&args(&["-p", "", "mybin"])).is_none());
        assert!(parse_npx_args(&args(&["--package", "", "mybin"])).is_none());
    }

    #[test]
    fn parse_npx_args_rejects_empty_inline_package_value() {
        assert!(parse_npx_args(&args(&["--package=", "mybin"])).is_none());
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

    // npx-resolver.ts:132: same `!value` empty-string gap as `parseNpxArgs`, ported to the
    // `npm exec --package` form.
    #[test]
    fn parse_npm_exec_args_rejects_empty_package_flag_value() {
        assert!(parse_npm_exec_args(&args(&["exec", "--package", "", "--", "mybin"])).is_none());
    }

    fn name_of(spec: &str) -> Option<String> {
        parse_package_spec(spec).map(|parsed| parsed.package_name)
    }

    // The four `extractPackageName` cases, kept as-is against its replacement: MCP-105 removed the
    // function upstream but not the behaviour, and these are the rows that would regress if
    // `parse_package_spec` got the `@scope` split wrong.
    #[test]
    fn extract_package_name_scoped_with_version() {
        assert_eq!(name_of("@foo/bar@1.2.3").as_deref(), Some("@foo/bar"));
    }

    #[test]
    fn extract_package_name_scoped_no_version() {
        assert_eq!(name_of("@foo/bar").as_deref(), Some("@foo/bar"));
    }

    #[test]
    fn extract_package_name_unscoped_with_version() {
        assert_eq!(name_of("bar@1.2.3").as_deref(), Some("bar"));
    }

    #[test]
    fn extract_package_name_scoped_missing_slash_is_none() {
        assert!(name_of("@foo").is_none());
    }

    /// Every one of these 29 rows was produced by running upstream's own `parsePackageSpec`
    /// (`tmp/pi-mcp-adapter/npx-resolver.ts:304`, `v2.26.1` = `fafae21`) on node 22, over a copy of
    /// the module with the function re-exported; the copy was deleted afterwards and the upstream
    /// checkout verified clean. Nothing here is transcribed from the regex by eye — the three rows
    /// that would have been wrong if it were are marked.
    #[test]
    fn parse_package_spec_matches_upstream_case_for_case() {
        // (spec, packageName or None, exactVersion or None)
        let table: &[(&str, Option<&str>, Option<&str>)] = &[
            ("pkg", Some("pkg"), None),
            ("pkg@1.2.3", Some("pkg"), Some("1.2.3")),
            ("pkg@^1.2.0", Some("pkg"), None),
            ("pkg@~1.2.0", Some("pkg"), None),
            ("pkg@=1.2.3", Some("pkg"), Some("1.2.3")),
            ("pkg@v1.2.3", Some("pkg"), Some("1.2.3")),
            ("pkg@V1.2.3", Some("pkg"), Some("1.2.3")),
            ("pkg@=v1.2.3", Some("pkg"), Some("1.2.3")),
            ("pkg@1.2.3-beta.1", Some("pkg"), Some("1.2.3-beta.1")),
            ("pkg@1.2.3+build.5", Some("pkg"), Some("1.2.3+build.5")),
            ("pkg@1.2.3-rc.1+build.5", Some("pkg"), Some("1.2.3-rc.1+build.5")),
            ("pkg@1.2", Some("pkg"), None),
            ("pkg@latest", Some("pkg"), None),
            ("pkg@", Some("pkg"), None),
            ("pkg@1.2.3.4", Some("pkg"), None),
            // SURPRISE 1: leading zeros PIN, and pin to the literal string.
            ("pkg@01.2.3", Some("pkg"), Some("01.2.3")),
            ("@scope/name", Some("@scope/name"), None),
            ("@scope/name@1.2.3", Some("@scope/name"), Some("1.2.3")),
            ("@scope/name@^1.0.0", Some("@scope/name"), None),
            ("@scope", None, None),
            ("@", None, None),
            ("", None, None),
            // SURPRISE 2: the spec is trimmed before anything else.
            ("  spaced@1.0.0  ", Some("spaced"), Some("1.0.0")),
            ("pkg@-1.2.3", Some("pkg"), None),
            ("pkg@1.2.3-", Some("pkg"), None),
            ("pkg@1.2.3+", Some("pkg"), None),
            ("@scope/name@=V2.0.0", Some("@scope/name"), Some("2.0.0")),
            // SURPRISE 3: exactly ONE `v` is stripped, so `vv` pins nothing.
            ("pkg@vv1.2.3", Some("pkg"), None),
            ("pkg@ 1.2.3", Some("pkg"), None),
        ];
        for (spec, name, version) in table {
            let parsed = parse_package_spec(spec);
            assert_eq!(
                parsed.as_ref().map(|parsed| parsed.package_name.as_str()),
                *name,
                "packageName for {spec:?}"
            );
            assert_eq!(
                parsed
                    .as_ref()
                    .and_then(|parsed| parsed.exact_version.as_deref()),
                *version,
                "exactVersion for {spec:?}"
            );
        }
    }

    /// A `\d`-shaped trap Rust would fall into and JavaScript would not: `regex`'s `\d` is
    /// Unicode-aware, so a pattern written with `\d` would accept Arabic-Indic digits and pin a
    /// "version" npm cannot have produced. MEASURED against upstream on node 22:
    /// `parsePackageSpec("pkg@\u{661}.\u{662}.\u{663}")` is `{"packageName":"pkg"}` — no pin.
    ///
    /// The trailing-newline rows are the other half of the measurement and they corrected the
    /// comment above the pattern: `"pkg@1.2.3\n"` DOES pin upstream, to `"1.2.3"`, because
    /// `spec.trim()` removes the newline before the pattern ever sees it. The `\A`/`\z` anchors
    /// are therefore belt-and-braces rather than a fix for a real divergence — Rust's `$` does not
    /// match before a trailing newline either — and only an INTERIOR newline reaches the pattern.
    #[test]
    fn only_ascii_digits_can_pin_a_version() {
        let pin = |spec: &str| {
            parse_package_spec(spec)
                .expect("a name is still parsed")
                .exact_version
        };
        assert_eq!(pin("pkg@\u{661}.\u{662}.\u{663}"), None);
        // Trimmed before parsing, so this pins — upstream does the same.
        assert_eq!(pin("pkg@1.2.3\n").as_deref(), Some("1.2.3"));
        // An interior newline or tab is what the anchors actually exclude.
        assert_eq!(pin("pkg@1.2.3\nx"), None);
        assert_eq!(pin("pkg@1.2.3\tx"), None);
    }

    /// `npx-resolver.ts:60-66`'s fourth clause. The entry's own `packageVersion` is what decides,
    /// and an entry that never recorded one can never satisfy a pinned request.
    #[test]
    fn a_cache_entry_recorded_for_another_version_is_rejected() {
        let entry = NpxCacheEntry {
            // The predicate calls `existsSync`, so point it at something that exists.
            resolved_bin: std::env::current_exe()
                .expect("a test binary exists")
                .to_string_lossy()
                .into_owned(),
            resolved_at: 1_000,
            package_version: Some("2.0.0".to_string()),
            is_js: false,
        };
        // A range (no exact version) accepts whatever is cached — upstream's `!packageSpec?.exactVersion`.
        assert!(cache_entry_is_usable(&entry, None, 1_000));
        // The matching pin accepts.
        assert!(cache_entry_is_usable(&entry, Some("2.0.0"), 1_000));
        // A different pin rejects. THIS is the arm that did not exist before MCP-105.
        assert!(!cache_entry_is_usable(&entry, Some("1.0.0"), 1_000));
        // An entry with no recorded version can never satisfy a pin.
        let unversioned = NpxCacheEntry { package_version: None, ..entry.clone() };
        assert!(cache_entry_is_usable(&unversioned, None, 1_000));
        assert!(!cache_entry_is_usable(&unversioned, Some("1.0.0"), 1_000));
        // And the TTL still governs, pin or no pin.
        assert!(!cache_entry_is_usable(&entry, Some("2.0.0"), 1_000 + CACHE_TTL_MS));
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

    /// MCP-105's verify line, as a hermetic fixture: **two versions of the same package in `_npx`,
    /// with the WRONG one carrying the newer mtime**.
    ///
    /// Without the version filter, `findCachedPackageDir`'s newest-mtime-first walk returns the
    /// 2.0.0 copy for every request, so `npx -y pkg@1.0.0` launches 2.0.0 — silently, and for as
    /// long as that directory stays newest. The three assertions are the three behaviours: a pin
    /// selects by version, a RANGE still selects by mtime (upstream does not resolve ranges), and a
    /// pin for a version that is not installed resolves to nothing rather than to the nearest thing.
    #[cfg(unix)]
    #[test]
    fn an_exact_version_pins_past_a_newer_wrong_one() {
        let root = std::env::temp_dir().join(format!(
            "cyrup-npx-pin-{}-{}",
            std::process::id(),
            now_ms()
        ));

        let install = |hash: &str, version: &str| {
            let hash_dir = root.join("_npx").join(hash);
            let pkg_dir = hash_dir.join("node_modules").join("widget");
            fs::create_dir_all(&pkg_dir).expect("mkdir");
            fs::write(
                pkg_dir.join("package.json"),
                format!(r#"{{"name":"widget","version":"{version}","bin":{{"widget":"cli.js"}}}}"#),
            )
            .expect("package.json");
            fs::write(pkg_dir.join("cli.js"), "#!/usr/bin/env node\n").expect("cli.js");
            hash_dir
        };

        // 1.0.0 first, then 2.0.0 — so the 2.0.0 directory is the newer one and wins the mtime sort.
        let old_dir = install("aaa", "1.0.0");
        std::thread::sleep(Duration::from_millis(1100));
        let new_dir = install("bbb", "2.0.0");
        let mtime_of = |dir: &Path| {
            dir.metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(UNIX_EPOCH)
        };
        assert!(
            mtime_of(&new_dir) > mtime_of(&old_dir),
            "the fixture is only meaningful if the WRONG copy is the newer one"
        );

        // A range pins nothing, so the newest-mtime copy wins — upstream's unchanged behaviour.
        let ranged = resolve_from_npm_cache_at(&root, "widget@^1.0.0", None).expect("resolves");
        assert_eq!(ranged.package_version.as_deref(), Some("2.0.0"));
        assert!(ranged.resolved_bin.contains("bbb"));

        // An exact version pins past it. THIS is the bug MCP-105 closes.
        let pinned = resolve_from_npm_cache_at(&root, "widget@1.0.0", None).expect("resolves");
        assert_eq!(pinned.package_version.as_deref(), Some("1.0.0"));
        assert!(
            pinned.resolved_bin.contains("aaa"),
            "expected the 1.0.0 copy, got {}",
            pinned.resolved_bin
        );

        // A pin for something that is not installed resolves to NOTHING — never to the nearest.
        assert!(resolve_from_npm_cache_at(&root, "widget@3.0.0", None).is_none());

        let _ = fs::remove_dir_all(&root);
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

    /// [`resolve_npx_binary`] runs on separate real OS threads via `tokio::task::block_in_place`
    /// (`&self` `ProcCaps::spawn`, this module's top-level doc comment), so [`save_cache_entry`]'s
    /// tmp-path race is reachable with genuine concurrency, not just in theory. Drive many real
    /// `std::thread`s at [`save_cache_entry_at`] against the SAME fixture cache file concurrently,
    /// each inserting a distinct key — matching Pi's `saveCacheEntry` guarantee (synchronous, so
    /// Node's single-threaded event loop never interleaves two calls, npx-resolver.ts:387-408): if
    /// [`SAVE_CACHE_LOCK`] genuinely serializes the read-merge-write-rename cycle, every thread's
    /// insert survives (each sees the prior thread's write before its own read) and the final file
    /// is always valid, un-corrupted JSON. Before the fix (no lock) this test was flaky — some runs
    /// lost entries (classic read-old/merge/last-writer-wins TOCTOU) and some produced a tmp file
    /// whose content was a byte-interleaved mix of two threads' `serde_json::to_string_pretty`
    /// output that failed to parse at all.
    #[test]
    fn save_cache_entry_survives_concurrent_writers_from_real_threads() {
        let tmp = std::env::temp_dir().join(format!(
            "cyrup-npx-save-cache-concurrent-{}-{}",
            std::process::id(),
            now_ms()
        ));
        const N: usize = 16;
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let path = tmp.clone();
                std::thread::spawn(move || {
                    let entry = NpxCacheEntry {
                        resolved_bin: format!("/fake/bin/{i}"),
                        resolved_at: now_ms(),
                        package_version: None,
                        is_js: true,
                    };
                    save_cache_entry_at(&path, &format!("key-{i}"), &entry);
                })
            })
            .collect();
        for h in handles {
            h.join().expect("writer thread must not panic");
        }

        let raw = fs::read_to_string(&tmp).expect("final cache file must exist and be readable");
        let cache: NpxCache =
            serde_json::from_str(&raw).expect("final cache file must be valid, un-corrupted JSON");
        assert_eq!(
            cache.entries.len(),
            N,
            "every concurrent writer's distinct key must survive the serialized read-merge-write \
             cycle, got: {:?}",
            cache.entries.keys().collect::<Vec<_>>()
        );
        for i in 0..N {
            assert!(cache.entries.contains_key(&format!("key-{i}")), "missing key-{i}");
        }

        let _ = fs::remove_file(&tmp);
    }
}
