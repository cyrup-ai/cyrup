//! The extension config `config.json` (port of pi `extension-config.ts`). The `enabled` master
//! switch, two toggles (`debug`, `yoloMode`) plus the forwarded-prompt timeout. JSONC (trailing
//! commas / comments allowed).
//!
//! Ports pi's `ensurePermissionSystemConfig` + `loadPermissionSystemConfig`
//! (`extension-config.ts:99-138`): a missing config file is auto-materialized on disk (pretty
//! `DEFAULT_EXTENSION_CONFIG` + trailing newline, parent dir created recursively) rather than only
//! defaulted in memory; the config path is overridable via
//! `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` (pi's `PI_PERMISSION_SYSTEM_CONFIG_PATH` /
//! `CONFIG_PATH_ENV_KEY`, `:43-49`), taking precedence over the caller-supplied default path (pi's
//! `configPath || overridePath || CONFIG_PATH` — the caller here never supplies an explicit
//! override, so this reduces to `env || default`); and a load failure is only silenced when it's
//! the expected "file absent" case (`formatJsoncConfigLoadWarning`'s ENOENT check,
//! `jsonc-config.ts:37-52`) — any other read/parse failure produces a warning (pi surfaces it via
//! `ctx.ui.notify`; wiring a host-UI channel here is out of scope for this module, so it is
//! `eprintln!`ed and also returned structurally via [`ExtensionConfigLoadResult`] for a future
//! caller to surface).
//!
//! Also ports pi's SAVE path, `savePermissionSystemConfig` (`extension-config.ts:240-293`) — see
//! [`ExtensionConfig::save`]. It is not a whole-file rewrite: the existing document is read,
//! merged into, and written back, so keys this extension does not own survive; a file that cannot
//! be parsed is refused rather than clobbered; and a symlinked config is written through to its
//! target.

use std::path::{Path, PathBuf};

use crate::jsonc;

/// pi `CONFIG_PATH_ENV_KEY` (`extension-config.ts:43`), renamed to this crate's `CYRUP_` env-var
/// convention (see `extension.rs::INSTALL_ENV_VAR`, `forwarding.rs::FORWARDING_AGENT_DIR_ENV`).
pub const CONFIG_PATH_ENV_KEY: &str = "CYRUP_PERMISSION_SYSTEM_CONFIG_PATH";

/// pi `PermissionSystemExtensionConfig` (`extension-config.ts:10-16`); defaults `{true, false,
/// false, Some(30.0)}` (`:29-34`).
///
/// No `Eq`: `forwarded_prompt_timeout_seconds` is an `Option<f64>` because upstream's field is a
/// JS `number | null` and a fractional value is a legal, preserved one (see that field's doc).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionConfig {
    /// pi `enabled` — "Master switch. When false, the extension skips all registrations and
    /// startup work." (`extension-config.ts:11-12`).
    ///
    /// Normalization is `record.enabled !== false` (`:88`): ONLY the literal boolean `false`
    /// disables. A missing key (every config file written before this key existed), `null`, `0`
    /// and the string `"false"` all leave it enabled — see [`ExtensionConfig::normalize`], which
    /// is deliberately an inequality against `false` rather than a truthiness test.
    ///
    /// The switch is honoured in `extension::permission_extension_for_env`, cyrup's analog of pi's
    /// `if (!extensionConfig.enabled) { return; }` early return from the extension entry point
    /// (`index.ts:1473-1477`).
    pub enabled: bool,
    /// pi `debug` — `strict === true` (`:89`).
    pub debug: bool,
    /// pi `yoloMode` — `strict === true` (`:90`). (Auto-approve of `ask` is P-1/P-3 territory; carried
    /// now for shape parity.)
    pub yolo_mode: bool,
    /// pi `forwardedPromptTimeoutSeconds`: `null`/`false` → `None` (indefinite); a finite `> 0`
    /// number → THAT NUMBER VERBATIM; else `Some(30.0)` (v0.8.0 `extension-config.ts:78-85`).
    /// Consumed by forwarding (P-4).
    ///
    /// `f64`, not an integer type. Upstream's declared type is `number | null`
    /// (`extension-config.ts:15`) and its keep-branch is `forwardedPromptTimeoutSeconds = rawTimeout`
    /// (`:84`) — the raw value, unrounded — so `45.5` survives normalization at `45.5`, is written
    /// back by `savePermissionSystemConfig` at `45.5` (`:202`), and becomes a `45.5 * 1000` = 45500 ms
    /// dialog timeout (`index.ts:1200-1201`). Holding it as `u64` truncated all three: an operator's
    /// `45.5` normalized to `45`, and because this key is one of the [`EXTENSION_CONFIG_KEYS`] that
    /// [`ExtensionConfig::save`] writes back, the first `/permission-system` toggle rewrote their
    /// file to `45`.
    ///
    /// The float is confined to this field's storage: [`ExtensionConfig::save`] re-narrows an
    /// integral value to an integer literal so `30` never becomes `30.0` on disk (see
    /// `timeout_json`), and forwarding builds a `Duration` with `try_from_secs_f64`.
    pub forwarded_prompt_timeout_seconds: Option<f64>,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        // pi `DEFAULT_EXTENSION_CONFIG` (`extension-config.ts:29-34`).
        ExtensionConfig {
            enabled: true,
            debug: false,
            yolo_mode: false,
            forwarded_prompt_timeout_seconds: Some(30.0),
        }
    }
}

/// pi `PermissionSystemConfigLoadResult` (`extension-config.ts:18-22`): the config plus whether
/// this call materialized the default file on disk and, if the load wasn't clean, why.
///
/// No `Eq`, for the same reason [`ExtensionConfig`] has none.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionConfigLoadResult {
    pub config: ExtensionConfig,
    pub created: bool,
    pub warning: Option<String>,
}

/// pi `PermissionSystemConfigSaveResult` (`extension-config.ts:24-27`): a save NEVER throws — a
/// failure is returned so the caller can surface it (pi `saveExtensionConfig` does
/// `ctx.ui.notify(saved.error, "error")`, `index.ts:1405-1410`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionConfigSaveResult {
    pub success: bool,
    pub error: Option<String>,
}

/// pi `EXTENSION_CONFIG_KEYS` (`extension-config.ts:144-148`): "Extension-managed keys that are
/// written/updated by `savePermissionSystemConfig`. All other keys in the config file are preserved
/// as-is."
///
/// Note upstream DELIBERATELY omits `enabled` from this list even though `enabled` IS part of its
/// `PermissionSystemExtensionConfig` (`extension-config.ts:11-12`) — a save must never inject that
/// key into a file that does not already carry it. It is omitted here for the same reason: writing
/// it into a legacy three-key file would make that file stop reading as pristine
/// ([`ExtensionConfig::is_pristine_default_file`]) and arm the install probe from the other
/// direction.
pub const EXTENSION_CONFIG_KEYS: [&str; 3] = ["debug", "yoloMode", "forwardedPromptTimeoutSeconds"];

/// Outcome of the disk-materialization step (pi `ensurePermissionSystemConfig`'s return shape,
/// `extension-config.ts:99-115`).
struct EnsureResult {
    created: bool,
    warning: Option<String>,
}

/// pi `readExistingConfig`'s return shape (`extension-config.ts:158-175`): the parsed record, plus
/// the distinction between "no file yet" (fine, write a fresh one) and "present but unreadable /
/// unparseable" (refuse — see [`ExtensionConfig::save`]).
enum ExistingConfig {
    /// pi `{ record: null, parseError: false }` (`:161-163`).
    Absent,
    /// pi `{ record, parseError: false }` (`:171`). Empty when the document parsed but is not an
    /// object, mirroring `toRecord`'s `{}` for non-records (`common.ts:7-13`).
    Parsed(Vec<(String, OrderedJson)>),
    /// pi `{ record: null, parseError: true }` (`:173`).
    Unparseable,
}

impl ExtensionConfig {
    /// pi `getPermissionSystemConfigPath(configPath?)` (`extension-config.ts:51-53`):
    /// `configPath || overridePath || CONFIG_PATH`. This crate's call site always supplies a
    /// computed default path (the analog of pi's `CONFIG_PATH`) rather than an optional explicit
    /// override, so the precedence collapses to `env (trimmed, non-empty) || default_path`.
    #[must_use]
    pub fn resolve_config_path(default_path: &Path) -> PathBuf {
        std::env::var(CONFIG_PATH_ENV_KEY)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| default_path.to_path_buf())
    }

    /// Load from `path` (JSONC), applying the env-var override and disk-materialization pi
    /// performs before every load. Kept as a plain `ExtensionConfig` return for source
    /// compatibility with the existing call site; use [`ExtensionConfig::load_with_result`] for
    /// the structured `{config, created, warning}` pi actually returns.
    #[must_use]
    pub fn load(path: &Path) -> ExtensionConfig {
        Self::load_with_result(path).config
    }

    /// pi `loadPermissionSystemConfig(configPath?)` (`extension-config.ts:117-138`): resolve the
    /// path, `ensurePermissionSystemConfig` it onto disk if absent, then read + parse + normalize,
    /// falling back to defaults with a warning (ENOENT-suppressed) on any failure.
    #[must_use]
    pub fn load_with_result(path: &Path) -> ExtensionConfigLoadResult {
        // Test-only load accounting; see `LOAD_COUNT`. No effect on a release build.
        #[cfg(test)]
        LOAD_COUNT.with(|count| count.set(count.get().saturating_add(1)));
        let resolved = Self::resolve_config_path(path);
        let ensure = Self::ensure_on_disk(&resolved);

        match std::fs::read_to_string(&resolved) {
            Ok(text) => {
                let subject = "permission-system config";
                let path_str = resolved.display().to_string();
                match jsonc::parse_config(&text, &path_str, subject) {
                    Ok(value) => ExtensionConfigLoadResult {
                        config: Self::normalize(&value),
                        created: ensure.created,
                        warning: ensure.warning,
                    },
                    Err(err) => {
                        // pi `formatJsoncConfigLoadWarning(configPath, error, subject,
                        // "using default extension config")` (`jsonc-config.ts:37-52`): a parse
                        // error is never an ENOENT, so it always gets the fallback suffix appended
                        // — unless `ensureResult.warning` already won (`extension-config.ts:133`).
                        let warning =
                            ensure.warning.unwrap_or_else(|| format!("{err}; using default extension config."));
                        eprintln!("cyrup-permission-system: warning: {warning}");
                        ExtensionConfigLoadResult {
                            config: ExtensionConfig::default(),
                            created: ensure.created,
                            warning: Some(warning),
                        }
                    }
                }
            }
            Err(err) => {
                // pi's ENOENT-suppression (`isNodeErrorWithCode(error, "ENOENT")` →
                // `formatJsoncConfigLoadWarning` returns `null`): the "file absent" case is
                // expected/silent. Any other read failure (permission denied, EISDIR, ...) is
                // surfaced. `ensureResult.warning` (a failed materialize-to-disk) always wins over
                // either outcome (`extension-config.ts:133`).
                let warning = ensure.warning.or_else(|| {
                    if err.kind() == std::io::ErrorKind::NotFound {
                        None
                    } else {
                        Some(format!(
                            "Failed to load {subject} at '{path}': {err}; using default extension config.",
                            subject = "permission-system config",
                            path = resolved.display()
                        ))
                    }
                });
                if let Some(ref w) = warning {
                    eprintln!("cyrup-permission-system: warning: {w}");
                }
                ExtensionConfigLoadResult { config: ExtensionConfig::default(), created: ensure.created, warning }
            }
        }
    }

    /// pi `ensurePermissionSystemConfig(configPath)` (`extension-config.ts:99-115`): if `path`
    /// doesn't already exist, `mkdir -p` its parent and write the pretty-printed default config +
    /// trailing newline, leaving a real, editable template file on disk.
    fn ensure_on_disk(path: &Path) -> EnsureResult {
        if path.exists() {
            return EnsureResult { created: false, warning: None };
        }

        let write_result: std::io::Result<()> = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, Self::default_config_content())
        })();

        match write_result {
            Ok(()) => EnsureResult { created: true, warning: None },
            Err(err) => EnsureResult {
                created: false,
                warning: Some(format!(
                    "Failed to initialize permission-system config at '{}': {err}",
                    path.display()
                )),
            },
        }
    }

    /// pi `createDefaultConfigContent()` (`extension-config.ts:72-74`):
    /// `` `${JSON.stringify(DEFAULT_EXTENSION_CONFIG, null, 2)}\n` `` — field order/spacing built
    /// by hand (rather than via `serde_json`, whose default `Map` is alphabetically ordered) so the
    /// on-disk template matches pi's `enabled`/`debug`/`yoloMode`/`forwardedPromptTimeoutSeconds`
    /// declaration order (`extension-config.ts:29-34`) byte for byte.
    pub(crate) fn default_config_content() -> String {
        let default = ExtensionConfig::default();
        // `Display for f64` prints the shortest round-tripping form and omits a zero fraction, so
        // `30.0` renders `30` — byte-identical to `JSON.stringify(30)`. (`Debug` would print `30.0`;
        // do not swap it in.)
        let timeout = match default.forwarded_prompt_timeout_seconds {
            Some(seconds) => seconds.to_string(),
            None => "null".to_string(),
        };
        format!(
            "{{\n  \"enabled\": {},\n  \"debug\": {},\n  \"yoloMode\": {},\n  \"forwardedPromptTimeoutSeconds\": {timeout}\n}}\n",
            default.enabled, default.debug, default.yolo_mode
        )
    }

    /// The template [`Self::default_config_content`] produced in cyrup builds BEFORE `enabled` was
    /// ported, frozen as a literal.
    ///
    /// [`Self::is_pristine_default_file`] is a byte-exact compare, and it is what stops an
    /// auto-materialized `config.json` from latching the permission gate on forever (see that
    /// function's doc and `extension::is_installed`). Adding a fourth key to the template would
    /// therefore have re-armed the gate, silently and on upgrade, for every user who already had
    /// the three-key file on disk — precisely the population the latch fix was written for. So the
    /// probe accepts this legacy template too.
    ///
    /// Frozen as a `const` rather than regenerated from an older `Default`: the whole point is
    /// that it must keep describing the bytes that build actually wrote, however the current
    /// defaults later change.
    pub(crate) const LEGACY_DEFAULT_CONFIG_CONTENT: &str =
        "{\n  \"debug\": false,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n";

    /// Whether `path` currently holds a BYTE-EXACT pristine template
    /// [`Self::ensure_on_disk`] auto-materializes — i.e. nothing has edited it since this crate
    /// created it. "A" template, plural: the current one plus
    /// [`Self::LEGACY_DEFAULT_CONFIG_CONTENT`], the shape older cyrup builds wrote.
    ///
    /// This exists for the install probe (`extension::is_installed`). `config.json` is written by
    /// THIS crate as a side effect of merely constructing the extension, so its existence is a
    /// footprint of the code, not evidence that an operator asked for the gate; counting it
    /// latched the gate permanently on after a single opt-in run. Any deviation from the template
    /// IS evidence, because only a human (or another tool) writes those bytes.
    ///
    /// Deliberately conservative in the ambiguous direction: a present-but-unreadable file reports
    /// `false` (not pristine ⇒ treated as configured), because for a security gate the safe answer
    /// to "I cannot tell" is "assume it was configured". An ABSENT file also reports `false`, so
    /// callers must test existence separately (`is_installed` does).
    ///
    /// Accepting a SET of exact templates is additive and keeps both of those directions: nothing
    /// that read pristine before stops reading pristine, and nothing newly reads pristine except
    /// bytes this crate itself once wrote. (The looser alternative — parse the document and accept
    /// any key-subset that normalizes to the default — was rejected: a hand-authored
    /// `{"yoloMode": false}` would newly read as pristine and DISABLE a gate an operator
    /// deliberately configured.)
    #[must_use]
    pub fn is_pristine_default_file(path: &Path) -> bool {
        std::fs::read_to_string(path).is_ok_and(|text| {
            text == Self::default_config_content() || text == Self::LEGACY_DEFAULT_CONFIG_CONTENT
        })
    }

    /// pi `normalizePermissionSystemConfig` (v0.8.0 `extension-config.ts:76-93`).
    #[must_use]
    pub fn normalize(value: &serde_json::Value) -> ExtensionConfig {
        let default = ExtensionConfig::default();
        // pi `enabled: record.enabled !== false` (`:88`). NOT `as_bool().unwrap_or(true)`: that
        // would also disable on a non-boolean, where pi disables only on the literal `false`.
        let enabled = value.get("enabled") != Some(&serde_json::Value::Bool(false));
        let debug = value.get("debug").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let yolo_mode = value.get("yoloMode").and_then(serde_json::Value::as_bool).unwrap_or(false);

        let forwarded = match value.get("forwardedPromptTimeoutSeconds") {
            // `null` / `false` → indefinite.
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Bool(false)) => None,
            Some(v) => match v.as_f64() {
                // pi `:83-84` — `typeof rawTimeout === "number" && Number.isFinite(rawTimeout) &&
                // rawTimeout > 0` keeps `rawTimeout` ITSELF. No rounding, no flooring: a fractional
                // `45.5` is a finite positive number and upstream persists and uses it as `45.5`.
                Some(n) if n.is_finite() && n > 0.0 => Some(n),
                _ => default.forwarded_prompt_timeout_seconds,
            },
            None => default.forwarded_prompt_timeout_seconds,
        };

        ExtensionConfig { enabled, debug, yolo_mode, forwarded_prompt_timeout_seconds: forwarded }
    }

    /// The three extension-managed fields as a JSON object, so [`Self::save`] can run them back
    /// through [`Self::normalize`] exactly as pi does (`extension-config.ts:244`). pi normalizes on
    /// save because its input is an untyped object; the typed `ExtensionConfig` here is *nearly*
    /// normalized by construction, but not entirely — `forwarded_prompt_timeout_seconds: Some(0.0)`
    /// is representable and must be clamped to the `Some(30.0)` default, which only `normalize` does.
    ///
    /// `enabled` is deliberately absent: it is not in [`EXTENSION_CONFIG_KEYS`], so `save` never
    /// reads it off the round-tripped value and never writes it (see that constant's doc).
    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "debug": self.debug,
            "yoloMode": self.yolo_mode,
            "forwardedPromptTimeoutSeconds": self.forwarded_prompt_timeout_seconds,
        })
    }

    /// pi `normalizePermissionSystemConfig(next)` applied to an already-typed config — the FIRST
    /// statement of both v0.8.0 config WRITERS: `saveExtensionConfig` (`index.ts:1403`) and
    /// `setYoloModeFromRuntimeApi` (`index.ts:1432`, over `{ ...extensionConfig, yoloMode: enabled }`).
    ///
    /// Distinct from the private `to_value` round-trip [`Self::save`] performs, in exactly one way
    /// that matters: this one carries `enabled` through. `to_value` omits it deliberately (it is not
    /// an extension-managed FILE key), but the value both writers install as the new IN-MEMORY
    /// config is `normalizePermissionSystemConfig`'s output over the whole object, `enabled`
    /// included — so normalizing through `to_value` here would silently re-enable a gate whose
    /// config says `"enabled": false`.
    ///
    /// Not a no-op on a typed value: `forwarded_prompt_timeout_seconds: Some(0.0)` is representable
    /// and is clamped to the `Some(30.0)` default, the same reason `save` normalizes.
    #[must_use]
    pub fn normalized(&self) -> ExtensionConfig {
        Self::normalize(&serde_json::json!({
            "enabled": self.enabled,
            "debug": self.debug,
            "yoloMode": self.yolo_mode,
            "forwardedPromptTimeoutSeconds": self.forwarded_prompt_timeout_seconds,
        }))
    }

    /// pi `savePermissionSystemConfig(config, configPath?)` (`extension-config.ts:240-293`):
    /// atomically persist the three extension-managed keys **into** the existing config document
    /// rather than over it.
    ///
    /// Three behaviours are the point of this function, and all three were added upstream in the
    /// v0.8.0 rewrite of a save that used to `JSON.stringify` the normalized config over the whole
    /// file (v0.7.1 `extension-config.ts:132-159`):
    ///
    /// 1. **Non-extension keys are preserved.** The file is read and parsed first; every key it
    ///    holds that this extension does not own — `defaultPolicy`, `tools`, `bash`, `mcp`,
    ///    `skills`, `$schema`, anything a future version adds — is carried through untouched, and
    ///    the document's original key ORDER is kept (an extension key already present is updated in
    ///    place; a missing one is appended). pi `mergeExtensionFields` (`:186-216`).
    /// 2. **A corrupt file is never overwritten.** If the file exists but cannot be read or parsed,
    ///    the save FAILS rather than replacing salvageable permission data with three defaults. pi
    ///    `:249-257`.
    /// 3. **A symlinked config is written through, not replaced.** `tmp`+`rename` onto the symlink
    ///    path itself would swap the link for a regular file; the write target is therefore the
    ///    realpath when the config path is a symlink. pi `resolveWriteTarget` (`:223-238`).
    ///
    /// Like [`Self::load_with_result`], `path` is the caller-supplied DEFAULT: the
    /// [`CONFIG_PATH_ENV_KEY`] override still wins (pi `getPermissionSystemConfigPath`,
    /// `extension-config.ts:51-53`).
    #[must_use]
    pub fn save(&self, path: &Path) -> ExtensionConfigSaveResult {
        let resolved = Self::resolve_config_path(path);
        // pi `:244` — see `to_value`.
        let normalized = Self::normalize(&self.to_value());

        // pi `:247-257`: read first, and refuse outright on a parse error.
        let mut merged = match Self::read_existing(&resolved) {
            ExistingConfig::Unparseable => {
                return ExtensionConfigSaveResult {
                    success: false,
                    error: Some(format!(
                        "Refusing to save permission-system config at '{}': the existing file is \
                         corrupt or unparseable. Manual intervention is required to preserve \
                         existing permission data.",
                        resolved.display()
                    )),
                };
            }
            // pi `const baseRecord = existing.record ?? {}` (`:260`).
            ExistingConfig::Absent => Vec::new(),
            ExistingConfig::Parsed(entries) => entries,
        };

        // pi `mergeExtensionFields`'s prototype-pollution filter (`:190-197`). In JavaScript
        // `merged["__proto__"] = v` mutates the prototype chain instead of adding an own property;
        // Rust has no prototype chain and these are ordinary `String` keys in a `Vec`, so this is
        // NOT a security fix here. It is ported because it is OBSERVABLE: upstream's saved document
        // drops those keys and cyrup's would otherwise keep them.
        merged.retain(|(key, _)| !crate::common::is_prototype_pollution_key(key));

        for key in EXTENSION_CONFIG_KEYS {
            // pi `normalizedRecord` (`:199-203`) + the in-place-or-append loop (`:205-213`).
            let value = match key {
                "debug" => OrderedJson::Bool(normalized.debug),
                "yoloMode" => OrderedJson::Bool(normalized.yolo_mode),
                _ => match normalized.forwarded_prompt_timeout_seconds {
                    Some(seconds) => timeout_json(seconds),
                    None => OrderedJson::Null,
                },
            };
            OrderedJson::upsert(&mut merged, key, value);
        }

        // pi `` `${JSON.stringify(merged, null, 2)}\n` `` (`:269`).
        let mut text = String::new();
        OrderedJson::Object(merged).write_pretty(&mut text, 0);
        text.push('\n');

        let write_path = Self::resolve_write_target(&resolved);
        // pi `` const tmpPath = `${writePath}.tmp` `` (`:265`). Built on the `OsString` so a
        // non-UTF-8 path is not mangled by a `Display` round-trip.
        let tmp_path = {
            let mut tmp = write_path.clone().into_os_string();
            tmp.push(".tmp");
            PathBuf::from(tmp)
        };

        // pi `:267-271`: mkdir -p, write temp, rename over the target.
        let write_result: std::io::Result<()> = (|| {
            if let Some(parent) = write_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&tmp_path, text.as_bytes())?;
            std::fs::rename(&tmp_path, &write_path)
        })();

        match write_result {
            Ok(()) => ExtensionConfigSaveResult { success: true, error: None },
            Err(err) => {
                // pi `:273-285`: best-effort temp cleanup; the primary error is what's reported.
                let _ = std::fs::remove_file(&tmp_path);
                ExtensionConfigSaveResult {
                    success: false,
                    error: Some(format!(
                        "Failed to save permission-system config at '{}': {err}",
                        resolved.display()
                    )),
                }
            }
        }
    }

    /// pi `readExistingConfig(configPath)` (`extension-config.ts:158-175`): absent → write fresh;
    /// unreadable/unparseable → the caller MUST NOT overwrite; otherwise the parsed record.
    fn read_existing(path: &Path) -> ExistingConfig {
        if !path.exists() {
            return ExistingConfig::Absent;
        }
        // pi's `readFileSync` throw lands in the same `catch` as a parse error (`:172-174`), so a
        // present-but-unreadable file is `parseError: true` too.
        let Ok(raw) = std::fs::read_to_string(path) else {
            return ExistingConfig::Unparseable;
        };
        // pi `:167-168`: strip a UTF-8 BOM so the JSONC parser can handle it. Rust's
        // `read_to_string` likewise keeps a leading U+FEFF in the string.
        let text = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str());
        let path_str = path.display().to_string();
        match crate::jsonc::parse_config_into::<OrderedJson>(
            text,
            &path_str,
            "permission-system config",
        ) {
            Ok(OrderedJson::Object(entries)) => ExistingConfig::Parsed(entries),
            // pi `toRecord(parsed)` (`:170`, `common.ts:7-13`): a non-object document (array,
            // scalar, `null`) yields `{}` — NOT a parse error.
            Ok(_) => ExistingConfig::Parsed(Vec::new()),
            Err(_) => ExistingConfig::Unparseable,
        }
    }

    /// pi `resolveWriteTarget(configPath)` (`extension-config.ts:223-238`): when the config path is
    /// a symlink, write through to its realpath so `tmp`+`rename` updates the target instead of
    /// replacing the link with a regular file. Every failure path (missing file, broken link,
    /// unresolvable realpath) falls back to `configPath` itself, exactly as upstream's
    /// `catch`/fall-through do.
    fn resolve_write_target(path: &Path) -> PathBuf {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
            }
            _ => path.to_path_buf(),
        }
    }
}

/// A **lossless, insertion-order-preserving** JSON document, used only by
/// [`ExtensionConfig::save`] to read a config file and write it back with the extension-managed
/// keys updated in place.
///
/// Neither existing type in this crate can do that job:
/// - [`serde_json::Value`] stores objects in a `BTreeMap` (the workspace does not enable
///   `preserve_order`), so a round-trip would silently re-alphabetize the operator's file.
/// - [`crate::ordered::OrderedValue`] preserves order but is deliberately LOSSY — numbers, bools,
///   nulls and arrays all collapse to `OrderedValue::Other`, which cannot be written back out.
///
/// Preserving the user's key order is an explicit upstream requirement (v0.8.0
/// `tests/config-preservation-red.test.ts:1779` "preserves the original key ordering of the config
/// file"), as is preserving unknown keys with their values intact.
#[derive(Debug, Clone, PartialEq)]
enum OrderedJson {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    Str(String),
    Array(Vec<OrderedJson>),
    Object(Vec<(String, OrderedJson)>),
}

impl OrderedJson {
    /// JS object-assignment semantics: an existing key is updated IN PLACE (keeping its original
    /// position, pi `:206-209`), a new key is appended (pi `:210-212`).
    fn upsert(entries: &mut Vec<(String, OrderedJson)>, key: &str, value: OrderedJson) {
        if let Some(slot) = entries.iter_mut().find(|(existing, _)| existing == key) {
            slot.1 = value;
        } else {
            entries.push((key.to_string(), value));
        }
    }

    /// `JSON.stringify(value, null, 2)`: two-space indentation, `": "` after each key, no trailing
    /// whitespace, `{}` / `[]` for empty containers.
    ///
    /// [CYRUP-DELTA] number formatting: a float that happens to be integral round-trips as `1.0`
    /// here where V8 prints `1`, because `serde_json::Number` remembers that the input was a float.
    /// This can only affect a preserved non-extension key whose source text was written that way;
    /// the three keys this function actually updates are two booleans and an integer/`null`.
    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            OrderedJson::Null => out.push_str("null"),
            OrderedJson::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            OrderedJson::Number(value) => out.push_str(&value.to_string()),
            OrderedJson::Str(value) => out.push_str(&encode_json_string(value)),
            OrderedJson::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(",\n");
                    }
                    push_indent(out, indent + 1);
                    item.write_pretty(out, indent + 1);
                }
                out.push('\n');
                push_indent(out, indent);
                out.push(']');
            }
            OrderedJson::Object(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push_str(",\n");
                    }
                    push_indent(out, indent + 1);
                    out.push_str(&encode_json_string(key));
                    out.push_str(": ");
                    value.write_pretty(out, indent + 1);
                }
                out.push('\n');
                push_indent(out, indent);
                out.push('}');
            }
        }
    }
}

/// JS `Number.MAX_SAFE_INTEGER` (2^53 − 1) — the largest integer a JS `number` represents exactly,
/// and therefore the ceiling above which [`timeout_json`] can no longer claim its integer rendering
/// is what `JSON.stringify` would have produced.
const JS_MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Render `forwardedPromptTimeoutSeconds` the way `JSON.stringify` renders a JS `number`
/// (v0.8.0 `extension-config.ts:202` feeding `:269`'s stringify).
///
/// JS has ONE number type and prints an integral value with no fractional part — `30`, never `30.0`.
/// `serde_json::Number::from_f64(30.0)` remembers the value arrived as a float and prints `30.0`,
/// which would rewrite the `"forwardedPromptTimeoutSeconds": 30` in every existing operator file on
/// the first `/permission-system` toggle. So an integral, exactly-representable value is re-narrowed
/// to an integer `Number`; a genuinely fractional one (`45.5`) stays a float and prints `45.5`.
fn timeout_json(seconds: f64) -> OrderedJson {
    if seconds.fract() == 0.0 && (0.0..=JS_MAX_SAFE_INTEGER).contains(&seconds) {
        return OrderedJson::Number((seconds as u64).into());
    }
    // `from_f64` is `None` only for NaN/infinity, which `normalize`'s `is_finite()` already excluded.
    serde_json::Number::from_f64(seconds).map_or(OrderedJson::Null, OrderedJson::Number)
}

/// A JSON string literal with the standard escaping. `Display for serde_json::Value` is infallible
/// (it formats into a `String`), so this needs no fallible unwrap.
fn encode_json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

impl<'de> serde::Deserialize<'de> for OrderedJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(OrderedJsonVisitor)
    }
}

struct OrderedJsonVisitor;

impl<'de> serde::de::Visitor<'de> for OrderedJsonVisitor {
    type Value = OrderedJson;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(OrderedJson::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(OrderedJson::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(OrderedJson::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(OrderedJson::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(OrderedJson::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        // `from_f64` is `None` only for NaN/infinity, which JSON cannot express.
        Ok(serde_json::Number::from_f64(value).map_or(OrderedJson::Null, OrderedJson::Number))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(OrderedJson::Str(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(OrderedJson::Str(value))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(OrderedJson::Array(items))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut entries: Vec<(String, OrderedJson)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, OrderedJson>()? {
            // JS object semantics for a duplicate key: the LAST value wins, at the FIRST
            // position — which is exactly `Object.keys()` order after `JSON.parse`.
            OrderedJson::upsert(&mut entries, &key, value);
        }
        Ok(OrderedJson::Object(entries))
    }
}

/// Guards every test in this crate that mutates process-wide environment state
/// (`CONFIG_PATH_ENV_KEY` here, `extension::INSTALL_ENV_VAR` there) from running concurrently with
/// any other such test. Lives outside the `tests` module (and is `pub(crate)`) precisely so the
/// *same* lock instance serializes both modules — a per-module lock would not, and `cargo test`
/// runs the whole crate's unit tests in one process.
#[cfg(test)]
pub(crate) fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
thread_local! {
    /// How many times [`ExtensionConfig::load_with_result`] has run **on the current thread**.
    ///
    /// Test-only instrumentation for "the extension config is read exactly once per session build"
    /// (pi loads it once, `index.ts:1473`). The operator-visible symptom of loading twice is the
    /// duplicated `eprintln!` a malformed config produces, and `eprintln!` cannot be captured from
    /// inside the same process without redirecting fd 2, so the test counts the loads instead.
    ///
    /// Thread-LOCAL, not a global counter: `cargo test` runs this crate's unit tests as parallel
    /// threads in one process and many of them load a config, so a global would be raced by
    /// unrelated tests that hold no lock. A synchronous call chain stays on one thread, so the
    /// counter sees exactly the loads the call under test performed.
    static LOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Zero the current thread's [`LOAD_COUNT`].
#[cfg(test)]
pub(crate) fn reset_load_count() {
    LOAD_COUNT.with(|count| count.set(0));
}

/// Read the current thread's [`LOAD_COUNT`].
#[cfg(test)]
pub(crate) fn load_count() -> usize {
    LOAD_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn absent_is_defaults() {
        // Any test that resolves a config path reads `CONFIG_PATH_ENV_KEY`, so it must hold the
        // same lock the env-mutating test takes — cargo runs these as parallel threads in one
        // process, and without this the override leaks across tests intermittently.
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert_eq!(ExtensionConfig::load(&path), ExtensionConfig::default());
    }

    // Regression test for pi `ensurePermissionSystemConfig` (`extension-config.ts:99-115`):
    // pre-fix, `load` never wrote anything to disk on a missing config, so `config.json` never
    // existed unless something external created it. Loading an absent path must now materialize a
    // real, editable default-config template file at that path (mkdir -p'ing the parent first).
    #[test]
    fn absent_config_is_materialized_on_disk() {
        // Any test that resolves a config path reads `CONFIG_PATH_ENV_KEY`, so it must hold the
        // same lock the env-mutating test takes — cargo runs these as parallel threads in one
        // process, and without this the override leaks across tests intermittently.
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        assert!(!path.exists());

        let result = ExtensionConfig::load_with_result(&path);

        assert!(result.created, "first load of an absent config must report created: true");
        assert!(result.warning.is_none());
        assert_eq!(result.config, ExtensionConfig::default());
        assert!(path.exists(), "config.json must now exist on disk");

        let written = std::fs::read_to_string(&path).unwrap();
        // pi `createDefaultConfigContent()` = `JSON.stringify(DEFAULT_EXTENSION_CONFIG, null, 2)`
        // (`extension-config.ts:72-74`), and `DEFAULT_EXTENSION_CONFIG` (`:29-34`) declares
        // `enabled` FIRST — so a fresh file is four keys with `enabled` leading.
        assert_eq!(
            written,
            "{\n  \"enabled\": true,\n  \"debug\": false,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n"
        );

        // A second load must see the just-created file rather than re-creating it.
        let second = ExtensionConfig::load_with_result(&path);
        assert!(!second.created);
        assert!(second.warning.is_none());
    }

    // pi `enabled: record.enabled !== false` (`extension-config.ts:88`). ONLY the literal boolean
    // `false` disables; every other value — including the JS-falsy ones that are not `false` —
    // leaves the master switch on. The trap this pins is a "simplification" to a truthiness test
    // (`is_truthy(v)`, or `debug`'s own `as_bool().unwrap_or(false)` shape), either of which would
    // disable the gate on `null` / `0` / `""` where pi keeps it enabled. Pin the whole table.
    #[test]
    fn only_the_literal_false_disables_the_enabled_master_switch() {
        let enabled_of = |v: serde_json::Value| ExtensionConfig::normalize(&v).enabled;

        // The one disabling form.
        assert!(!enabled_of(serde_json::json!({"enabled": false})));

        // Everything else stays enabled.
        assert!(enabled_of(serde_json::json!({})), "a missing key is enabled (v0.7.1 files)");
        assert!(enabled_of(serde_json::json!({"enabled": true})));
        assert!(enabled_of(serde_json::json!({"enabled": null})));
        assert!(enabled_of(serde_json::json!({"enabled": 0})), "JS `0` is falsy but is not `false`");
        assert!(enabled_of(serde_json::json!({"enabled": "false"})), "the STRING is not `false`");
        assert!(enabled_of(serde_json::json!({"enabled": ""})));
        assert!(enabled_of(serde_json::json!({"enabled": []})));
        assert!(ExtensionConfig::default().enabled, "pi `DEFAULT_EXTENSION_CONFIG.enabled` (`:30`)");
    }

    #[test]
    fn null_timeout_is_indefinite_and_bools_parse() {
        let v = serde_json::json!({"debug": true, "yoloMode": true, "forwardedPromptTimeoutSeconds": null});
        let c = ExtensionConfig::normalize(&v);
        assert!(c.debug && c.yolo_mode);
        assert_eq!(c.forwarded_prompt_timeout_seconds, None);
    }

    #[test]
    fn finite_positive_timeout_kept_else_default() {
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": 45}))
                .forwarded_prompt_timeout_seconds,
            Some(45.0)
        );
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": -5}))
                .forwarded_prompt_timeout_seconds,
            Some(30.0)
        );
    }

    // F2 — regression test for pi `:83-84`: the keep-branch is `forwardedPromptTimeoutSeconds =
    // rawTimeout`, the raw finite-positive number, with no rounding step anywhere. Cyrup's
    // `Some(n as u64)` truncated it, and because `forwardedPromptTimeoutSeconds` is one of the
    // `EXTENSION_CONFIG_KEYS` that `save` writes back, the truncation did not stay in memory: the
    // first `/permission-system` toggle REWROTE the operator's `45.5` to `45` on disk.
    #[test]
    fn a_fractional_timeout_survives_normalize_and_a_save_round_trip() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        // 1. normalize keeps it verbatim.
        assert_eq!(
            ExtensionConfig::normalize(&serde_json::json!({"forwardedPromptTimeoutSeconds": 45.5}))
                .forwarded_prompt_timeout_seconds,
            Some(45.5),
            "a finite positive number is kept as-is (pi `extension-config.ts:84`)"
        );

        // 2. It survives a save/load round trip, and the BYTES on disk still say 45.5.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{\n  \"forwardedPromptTimeoutSeconds\": 45.5\n}\n").unwrap();

        let loaded = ExtensionConfig::load(&path);
        assert_eq!(loaded.forwarded_prompt_timeout_seconds, Some(45.5));

        // A `debug` toggle is what the human actually does; the timeout must ride along untouched.
        let saved = ExtensionConfig { debug: true, ..loaded }.save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"forwardedPromptTimeoutSeconds\": 45.5"),
            "an unrelated toggle must not rewrite the operator's timeout; got file:\n{text}"
        );
        assert_eq!(
            ExtensionConfig::load(&path).forwarded_prompt_timeout_seconds,
            Some(45.5),
            "the saved config must load back to what was saved"
        );
    }

    // MIRROR (must stay green): widening the field to `f64` must not make WHOLE seconds render as
    // floats. `JSON.stringify(30)` is `30`; `serde_json::Number::from_f64(30.0)` would print `30.0`,
    // which would rewrite every existing operator file on the first save (and, via
    // `is_pristine_default_file`'s byte-exact compare, would break the install probe's template).
    #[test]
    fn a_whole_second_timeout_is_still_written_as_an_integer_literal() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"debug\": true,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n",
            "a whole-second timeout must serialize as `30`, not `30.0`"
        );

        // The auto-materialized template is the same bytes, which is what the install probe compares
        // against (`is_pristine_default_file`).
        assert_eq!(
            ExtensionConfig::default_config_content(),
            "{\n  \"enabled\": true,\n  \"debug\": false,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n"
        );
    }

    // Regression test for pi `formatJsoncConfigLoadWarning` (`jsonc-config.ts:37-52`) as used by
    // `loadPermissionSystemConfig` (`extension-config.ts:129-137`): a malformed-but-present config
    // must produce a warning shaped like pi's (`Failed to parse ... at '...' (...); using default
    // extension config.`), not the old bespoke "is not valid config JSON" message, and must be
    // returned structurally (not only `eprintln!`ed) so a caller can surface it.
    #[test]
    fn malformed_present_config_warns_like_pi_and_falls_back_to_defaults() {
        // Any test that resolves a config path reads `CONFIG_PATH_ENV_KEY`, so it must hold the
        // same lock the env-mutating test takes — cargo runs these as parallel threads in one
        // process, and without this the override leaks across tests intermittently.
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{not json").unwrap();

        let result = ExtensionConfig::load_with_result(&path);

        assert_eq!(result.config, ExtensionConfig::default());
        assert!(!result.created, "a pre-existing file must not be reported as created");
        let warning = result.warning.expect("malformed JSON must produce a warning");
        assert!(
            warning.starts_with("Failed to parse permission-system config at"),
            "unexpected warning: {warning}"
        );
        assert!(warning.ends_with("using default extension config."), "unexpected warning: {warning}");
    }

    // Regression test for pi's ENOENT-only suppression in `formatJsoncConfigLoadWarning`
    // (`jsonc-config.ts:43-45`) vs. `formatJsoncConfigLoadWarning` on any OTHER read failure: an
    // absent file is silent (already covered by `absent_is_defaults`); a present-but-unreadable
    // file (e.g. a directory sitting at the config path) is NOT ENOENT and must produce a warning
    // instead of being silently swallowed like the pre-fix blanket `Ok(text) else return default`.
    #[test]
    fn present_but_unreadable_config_warns_instead_of_silent_default() {
        // Any test that resolves a config path reads `CONFIG_PATH_ENV_KEY`, so it must hold the
        // same lock the env-mutating test takes — cargo runs these as parallel threads in one
        // process, and without this the override leaks across tests intermittently.
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        // A directory at the config path exists (so `ensure_on_disk` does not try to create it)
        // but cannot be read as a file, giving a non-ENOENT `io::Error`.
        let path = dir.path().join("config.json");
        std::fs::create_dir(&path).unwrap();

        let result = ExtensionConfig::load_with_result(&path);

        assert_eq!(result.config, ExtensionConfig::default());
        assert!(!result.created);
        let warning = result.warning.expect("a non-ENOENT read failure must produce a warning, not silence");
        assert!(warning.contains("using default extension config."), "unexpected warning: {warning}");
    }

    // Regression test for pi `getPermissionSystemConfigPath` (`extension-config.ts:51-53`):
    // `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` (this crate's analog of pi's
    // `PI_PERMISSION_SYSTEM_CONFIG_PATH`) must override the caller-supplied default path. Pre-fix,
    // no environment variable was ever consulted anywhere in the crate.
    #[test]
    fn env_var_overrides_default_config_path() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let overridden = dir.path().join("overridden.json");
        std::fs::write(&overridden, r#"{"debug": true}"#).unwrap();
        let default_path = dir.path().join("default.json");

        // SAFETY: serialized by `env_lock` so no other test observes a partial mutation; restored
        // before returning.
        unsafe {
            std::env::set_var(CONFIG_PATH_ENV_KEY, overridden.display().to_string());
        }
        let result = ExtensionConfig::load(&default_path);
        unsafe {
            std::env::remove_var(CONFIG_PATH_ENV_KEY);
        }

        assert!(result.debug, "env-var override path must win over the caller-supplied default");
        assert!(!default_path.exists(), "the un-used default path must not be touched");
    }

    // ---------------------------------------------------------------------------------------
    // pi `savePermissionSystemConfig` (`extension-config.ts:240-293`).
    // ---------------------------------------------------------------------------------------

    /// A config file shaped like upstream's `ISSUE_CONFIG_WITH_PERMISSIONS`
    /// (v0.8.0 `tests/config-preservation-red.test.ts`): the extension's three keys living
    /// alongside the permission policy and an unknown/future key, in a deliberate NON-alphabetical
    /// order so a `BTreeMap` round-trip would be visible.
    const CONFIG_WITH_PERMISSIONS: &str = r#"{
  "$schema": "https://example.invalid/permissions.json",
  "defaultPolicy": "ask",
  "yoloMode": false,
  "bash": {
    "git *": "allow",
    "rm -rf /": "deny"
  },
  "tools": {
    "read": "allow"
  },
  "debug": false,
  "aFutureKey": [1, "two", {"three": true}],
  "forwardedPromptTimeoutSeconds": 30
}
"#;

    fn read_saved(path: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(path).unwrap();
        crate::jsonc::parse(&text).unwrap()
    }

    // Regression test for pi `savePermissionSystemConfig` + `mergeExtensionFields`
    // (`extension-config.ts:240-293`, `:186-216`). Pre-fix cyrup had NO save path at all; the
    // naive port (and pi's own v0.7.1 `extension-config.ts:132-159`) writes
    // `JSON.stringify(normalized)` over the whole file, which DESTROYS every permission rule and
    // every unknown key the operator put there. Toggling `debug` must not cost the user their
    // policy.
    #[test]
    fn save_preserves_non_extension_keys_and_their_order() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, CONFIG_WITH_PERMISSIONS).unwrap();

        let loaded = ExtensionConfig::load(&path);
        let saved = ExtensionConfig { debug: true, ..loaded }.save(&path);
        assert_eq!(saved, ExtensionConfigSaveResult { success: true, error: None });

        let raw = read_saved(&path);
        // The extension's own key was updated...
        assert_eq!(raw["debug"], serde_json::json!(true));
        // ...and every key the extension does NOT own survived, values intact.
        assert_eq!(raw["defaultPolicy"], serde_json::json!("ask"));
        assert_eq!(raw["$schema"], serde_json::json!("https://example.invalid/permissions.json"));
        assert_eq!(raw["bash"]["git *"], serde_json::json!("allow"));
        assert_eq!(raw["bash"]["rm -rf /"], serde_json::json!("deny"));
        assert_eq!(raw["tools"]["read"], serde_json::json!("allow"));
        assert_eq!(raw["aFutureKey"], serde_json::json!([1, "two", {"three": true}]));

        // And the operator's key ORDER is unchanged: `debug` was updated in place (position 6),
        // not moved to the end and not re-alphabetized (v0.8.0
        // `tests/config-preservation-red.test.ts:1779`).
        let text = std::fs::read_to_string(&path).unwrap();
        let keys: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("  \""))
            .filter_map(|line| line.split('"').next())
            .collect();
        assert_eq!(
            keys,
            vec![
                "$schema",
                "defaultPolicy",
                "yoloMode",
                "bash",
                "tools",
                "debug",
                "aFutureKey",
                "forwardedPromptTimeoutSeconds",
            ],
            "top-level key order must be preserved; got file:\n{text}"
        );
    }

    // Regression test for pi `:249-257` — "The file exists but cannot be parsed. We MUST NOT
    // overwrite it with only the extension fields, as that would destroy potentially salvageable
    // permission data." A v0.7.1-shaped save happily renames three default keys over a config the
    // user mistyped a comma into.
    #[test]
    fn save_refuses_to_overwrite_a_corrupt_config() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let corrupt = "{\n  \"defaultPolicy\": \"ask\"\n  \"bash\": { \"git *\": \"allow\" }\n}\n";
        std::fs::write(&path, corrupt).unwrap();

        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&path);

        assert!(!saved.success, "a corrupt config must not be saved over");
        let error = saved.error.expect("a refusal must carry an explanation");
        assert!(
            error.starts_with("Refusing to save permission-system config at"),
            "unexpected error: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "the corrupt file must be left byte-for-byte alone for manual repair"
        );
        assert!(!path.with_extension("json.tmp").exists(), "no temp file may be left behind");
    }

    // Regression test for pi `resolveWriteTarget` (`:223-238`) and v0.8.0
    // `tests/config-preservation-red.test.ts:1884` — a plain `tmp`+`rename` onto the config path
    // REPLACES a symlink with a regular file, silently detaching the user's config from wherever
    // they linked it (a dotfiles repo, a shared location). The write must go through to the target.
    #[cfg(unix)]
    #[test]
    fn save_writes_through_a_symlinked_config_instead_of_replacing_it() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-config.json");
        let link = dir.path().join("config.json");
        std::fs::write(&real, CONFIG_WITH_PERMISSIONS).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&link);
        assert!(saved.success, "save failed: {:?}", saved.error);

        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "config.json must still be a symlink after save"
        );
        let raw = read_saved(&real);
        assert_eq!(raw["debug"], serde_json::json!(true), "the link TARGET must be updated");
        assert_eq!(
            raw["defaultPolicy"],
            serde_json::json!("ask"),
            "permission keys must survive a write-through save"
        );
    }

    // pi `mergeExtensionFields`'s `__proto__`/`constructor`/`prototype` filter (`:190-197`), and
    // v0.8.0 `tests/config-preservation-red.test.ts:2024`. Parity, not a security fix: Rust has no
    // prototype chain (see the comment at the call site). What is observable is that upstream's
    // saved document does not contain these keys, and the permission keys beside them survive.
    #[test]
    fn save_drops_prototype_pollution_keys_and_keeps_the_rest() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"__proto__": {"polluted": true}, "constructor": {}, "prototype": 1,
                "defaultPolicy": "ask", "debug": false}"#,
        )
        .unwrap();

        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);

        let raw = read_saved(&path);
        let object = raw.as_object().expect("saved config must be an object");
        assert!(!object.contains_key("__proto__"));
        assert!(!object.contains_key("constructor"));
        assert!(!object.contains_key("prototype"));
        assert_eq!(raw["defaultPolicy"], serde_json::json!("ask"));
        assert_eq!(raw["debug"], serde_json::json!(true));
    }

    // MIRROR (must stay green): the corrupt-file refusal must not be over-broad. An ABSENT config
    // is not corrupt — pi returns `{ record: null, parseError: false }` for it (`:161-163`) and
    // writes a fresh document containing exactly the three extension keys and nothing invented.
    #[test]
    fn save_creates_an_absent_config_with_only_the_extension_keys() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");

        let saved = ExtensionConfig { yolo_mode: true, ..ExtensionConfig::default() }.save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"debug\": false,\n  \"yoloMode\": true,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n"
        );
    }

    // MIRROR (must stay green): "unparseable" means unparseable, not "not plain JSON". A config
    // carrying JSONC comments and trailing commas — which pi's parser and `jsonc.rs` both accept —
    // must save normally, with the comments understood to be dropped by the JSON round-trip
    // exactly as upstream's `JSON.stringify` drops them.
    #[test]
    fn save_over_a_jsonc_config_with_comments_is_not_treated_as_corrupt() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            "{\n  // the policy\n  \"defaultPolicy\": \"ask\",\n  \"debug\": false,\n}\n",
        )
        .unwrap();

        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);

        let raw = read_saved(&path);
        assert_eq!(raw["defaultPolicy"], serde_json::json!("ask"));
        assert_eq!(raw["debug"], serde_json::json!(true));
    }

    // MIRROR (must stay green): a UTF-8 BOM is not corruption either (pi `:167-168`).
    #[test]
    fn save_over_a_config_with_a_utf8_bom_is_not_treated_as_corrupt() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, format!("\u{feff}{CONFIG_WITH_PERMISSIONS}")).unwrap();

        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);
        assert_eq!(read_saved(&path)["defaultPolicy"], serde_json::json!("ask"));
    }

    // pi `:244` — the save normalizes its input, so an out-of-range timeout is clamped to the
    // default rather than persisted verbatim.
    #[test]
    fn save_normalizes_the_config_it_writes() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let saved = ExtensionConfig {
            forwarded_prompt_timeout_seconds: Some(0.0),
            debug: false,
            yolo_mode: false,
            enabled: true,
        }
        .save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);
        assert_eq!(read_saved(&path)["forwardedPromptTimeoutSeconds"], serde_json::json!(30));

        // `None` is a legitimate value (indefinite) and must round-trip as JSON `null`.
        let saved = ExtensionConfig { forwarded_prompt_timeout_seconds: None, ..ExtensionConfig::default() }
            .save(&path);
        assert!(saved.success, "save failed: {:?}", saved.error);
        assert_eq!(read_saved(&path)["forwardedPromptTimeoutSeconds"], serde_json::Value::Null);
        assert_eq!(
            ExtensionConfig::load(&path).forwarded_prompt_timeout_seconds,
            None,
            "a saved config must load back to what was saved"
        );
    }

    // pi `getPermissionSystemConfigPath` applies to the save path too (`:242`).
    #[test]
    fn save_honours_the_config_path_env_override() {
        let _guard = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let overridden = dir.path().join("overridden.json");
        let default_path = dir.path().join("default.json");

        // SAFETY: serialized by `env_lock`; restored before the assertions.
        unsafe {
            std::env::set_var(CONFIG_PATH_ENV_KEY, overridden.display().to_string());
        }
        let saved = ExtensionConfig { debug: true, ..ExtensionConfig::default() }.save(&default_path);
        unsafe {
            std::env::remove_var(CONFIG_PATH_ENV_KEY);
        }

        assert!(saved.success, "save failed: {:?}", saved.error);
        assert!(overridden.exists(), "the override path must be the one written");
        assert!(!default_path.exists(), "the un-used default path must not be touched");
    }
}
