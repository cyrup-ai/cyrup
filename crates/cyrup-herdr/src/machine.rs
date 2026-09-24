//! `herdr machine list --json` — herdr's catalog of saved SSH machines.
//!
//! herdr owns which machines exist and how ssh reaches them; a saved machine is "a label, SSH
//! target, explicit Herdr session, and enabled state" (`tmp/herdr/src/cli/machine.rs:17`). The
//! rows are printed by `list` (`tmp/herdr/src/cli/machine.rs:48-88`) from `MachineListRow`
//! (`:20-28`): `{id, label, target, session, enabled, selected}`, pretty-printed as one JSON array.
//!
//! The reader here is pi-subagents' (`src/runs/shared/herdr-machine.ts:98-137` @v0.68.0) rather
//! than a strict serde mirror of `MachineListRow`, because that is the contract saved-machine
//! placement was written against: it accepts either the bare array or `{machines: [...]}`, skips a
//! row with no usable `id`/`target` instead of failing the catalog, trims labels and sessions, and
//! reads `enabled` as "anything but `false`". The error sentences are pi's too, so an operator sees
//! the same words from cyrup as from pi.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::env::EnvSource;

/// `HERDR_MACHINE_LIST_TIMEOUT_MS` (`herdr-machine.ts:20`).
pub const MACHINE_LIST_TIMEOUT: Duration = Duration::from_millis(7_500);

/// `MAX_HERDR_MACHINE_LIST_BYTES` (`herdr-machine.ts:21`).
pub const MAX_MACHINE_LIST_BYTES: usize = 1024 * 1024;

/// One saved machine (`HerdrMachineCatalogEntry`, `herdr-machine.ts:30-36`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineProfile {
    /// herdr's profile id, trimmed and non-empty.
    pub id: String,
    /// The sidebar label, trimmed; `None` when absent or blank.
    pub label: Option<String>,
    /// The ssh target exactly as herdr stored it — validated by the caller, never trimmed here.
    pub target: String,
    /// The explicit remote herdr session, trimmed; `None` when absent or blank.
    pub session: Option<String>,
    /// `record.enabled !== false`.
    pub enabled: bool,
}

/// Why the catalog could not be read. `Display` is pi's sentence for each arm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// `ENOENT` spawning the binary (`herdr-machine.ts:128`).
    #[error(
        "Herdr CLI '{bin}' was not found on PATH. Saved-machine placement needs Herdr installed locally."
    )]
    NotInstalled {
        /// The binary that was tried.
        bin: String,
    },
    /// Any other failure to run it — spawn error, timeout, output over its bound (`:129`).
    #[error("Failed to run herdr machine list --json: {message}")]
    RunFailed {
        /// What went wrong.
        message: String,
    },
    /// A non-zero exit (`:131`). `output` is `(stderr || stdout).trim()`.
    #[error("herdr machine list --json exited with code {status}: {output}")]
    Exit {
        /// The exit code, or `null` rendered as `null` when a signal ended it.
        status: String,
        /// The trimmed stderr, else stdout.
        output: String,
    },
    /// stdout is not JSON (`:103`).
    #[error("Failed to parse herdr machine list --json: {message}")]
    Malformed {
        /// serde's message.
        message: String,
    },
    /// JSON, but neither an array nor `{machines: [...]}` (`:106`).
    #[error("herdr machine list --json returned no machine list.")]
    NoList,
}

/// `parseMachineCatalog(json)` (`herdr-machine.ts:98-117`).
///
/// # Errors
/// [`CatalogError::Malformed`] or [`CatalogError::NoList`].
pub fn parse_machine_catalog(json: &str) -> Result<Vec<MachineProfile>, CatalogError> {
    let parsed: serde_json::Value =
        serde_json::from_str(json).map_err(|error| CatalogError::Malformed {
            message: error.to_string(),
        })?;
    let entries = match &parsed {
        serde_json::Value::Array(entries) => entries,
        serde_json::Value::Object(object) => match object.get("machines") {
            Some(serde_json::Value::Array(entries)) => entries,
            _ => return Err(CatalogError::NoList),
        },
        _ => return Err(CatalogError::NoList),
    };
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let record = entry.as_object()?;
            let id = record.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let target = record.get("target")?.as_str()?;
            let trimmed = |key: &str| {
                record
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            Some(MachineProfile {
                id: id.to_string(),
                label: trimmed("label"),
                target: target.to_string(),
                session: trimmed("session"),
                enabled: record.get("enabled") != Some(&serde_json::Value::Bool(false)),
            })
        })
        .collect())
}

/// `readHerdrMachineCatalog(env, herdrBin)` (`herdr-machine.ts:119-133`): run
/// `<bin> machine list --json` under [`MACHINE_LIST_TIMEOUT`], its stdout bounded by
/// [`MAX_MACHINE_LIST_BYTES`], and parse it.
///
/// `bin` is resolved exactly as every other herdr call in this workspace resolves it —
/// `HERDR_BIN` then `herdr` ([`crate::HerdrCli::with_env`]) — when the caller passes `None`.
/// The child inherits the process environment, as upstream's `env ?? process.env` does.
///
/// # Errors
/// Every [`CatalogError`] arm.
pub async fn read_machine_catalog(
    bin: Option<&str>,
    env: &impl EnvSource,
) -> Result<Vec<MachineProfile>, CatalogError> {
    let bin = bin.map_or_else(
        || crate::HerdrCli::with_env(env).bin().to_string(),
        str::to_string,
    );
    let mut command = tokio::process::Command::new(&bin);
    command
        .args(["machine", "list", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CatalogError::NotInstalled { bin });
        }
        Err(error) => {
            return Err(CatalogError::RunFailed {
                message: error.to_string(),
            });
        }
    };
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(CatalogError::RunFailed {
            message: "the child's output pipes were not available".to_string(),
        });
    };
    let collect = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let limit = u64::try_from(MAX_MACHINE_LIST_BYTES).unwrap_or(u64::MAX);
        let mut out_pipe = (&mut stdout).take(limit.saturating_add(1));
        let mut err_pipe = (&mut stderr).take(limit.saturating_add(1));
        let (out_read, err_read) = tokio::join!(
            out_pipe.read_to_end(&mut out),
            err_pipe.read_to_end(&mut err),
        );
        out_read?;
        err_read?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((out, err, status))
    };
    let (out, err, status) = match tokio::time::timeout(MACHINE_LIST_TIMEOUT, collect).await {
        Ok(Ok(collected)) => collected,
        Ok(Err(error)) => {
            return Err(CatalogError::RunFailed {
                message: error.to_string(),
            });
        }
        Err(_) => {
            return Err(CatalogError::RunFailed {
                message: format!("timed out after {} ms", MACHINE_LIST_TIMEOUT.as_millis()),
            });
        }
    };
    if out.len() > MAX_MACHINE_LIST_BYTES || err.len() > MAX_MACHINE_LIST_BYTES {
        return Err(CatalogError::RunFailed {
            message: "stdout maxBuffer length exceeded".to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&out);
    if !status.success() {
        let stderr = String::from_utf8_lossy(&err);
        let output = if stderr.is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(CatalogError::Exit {
            status: status
                .code()
                .map_or_else(|| "null".to_string(), |code| code.to_string()),
            output,
        });
    }
    parse_machine_catalog(&stdout)
}
