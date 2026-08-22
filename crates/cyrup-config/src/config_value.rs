//! Config-value resolution language (Pi `resolve-config-value.ts`).
//!
//! Every stored / configured secret (an `auth.json` `api_key.key`, a `models.json` provider
//! `apiKey`, or a request header value) is treated as a tiny template:
//!
//! - A value starting with `!` is executed as a shell command; the trimmed stdout is the value and
//!   is **cached for the process lifetime** (Pi `executeCommand`, resolve-config-value.ts:208-216).
//! - Otherwise `$VAR` / `${VAR}` are interpolated from a provider-scoped `env` map, then the
//!   process environment (Pi `resolveEnvConfigValue`, :88-90). `$$` escapes a literal `$` and `$!`
//!   escapes a literal `!` (Pi `parseConfigValueTemplate`, :42-46).
//!
//! This is a faithful 1:1 port of `resolve-config-value.ts` (287 lines).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Process-lifetime cache of `!command` results (Pi `commandResultCache`, :10).
fn command_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `^[A-Za-z_][A-Za-z0-9_]*$` (Pi `ENV_VAR_NAME_RE`, :11).
fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Leading `[A-Za-z_][A-Za-z0-9_]*` run of `s` (Pi `ENV_VAR_NAME_PREFIX_RE`, :12).
fn env_var_name_prefix(s: &str) -> Option<&str> {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i == 0 {
            if c == '_' || c.is_ascii_alphabetic() {
                end = c.len_utf8();
            } else {
                return None;
            }
        } else if c == '_' || c.is_ascii_alphanumeric() {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 { None } else { Some(&s[..end]) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplatePart {
    Literal(String),
    Env(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConfigValueReference {
    Command(String),
    Template(Vec<TemplatePart>),
}

fn append_literal(parts: &mut Vec<TemplatePart>, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(TemplatePart::Literal(prev)) = parts.last_mut() {
        prev.push_str(value);
        return;
    }
    parts.push(TemplatePart::Literal(value.to_string()));
}

/// Port of `parseConfigValueTemplate` (resolve-config-value.ts:28-78).
fn parse_template(config: &str) -> Vec<TemplatePart> {
    let bytes = config.as_bytes();
    let mut parts: Vec<TemplatePart> = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        let dollar = config[index..].find('$').map(|i| index + i);
        let Some(dollar_index) = dollar else {
            append_literal(&mut parts, &config[index..]);
            break;
        };

        append_literal(&mut parts, &config[index..dollar_index]);
        let next_char = bytes.get(dollar_index + 1).copied();

        if next_char == Some(b'$') || next_char == Some(b'!') {
            // `$$` -> `$`, `$!` -> `!`.
            let lit = &config[dollar_index + 1..dollar_index + 2];
            append_literal(&mut parts, lit);
            index = dollar_index + 2;
            continue;
        }

        if next_char == Some(b'{') {
            let end = config[dollar_index + 2..]
                .find('}')
                .map(|i| dollar_index + 2 + i);
            let Some(end_index) = end else {
                append_literal(&mut parts, "$");
                index = dollar_index + 1;
                continue;
            };
            let name = &config[dollar_index + 2..end_index];
            if is_env_var_name(name) {
                parts.push(TemplatePart::Env(name.to_string()));
            } else {
                append_literal(&mut parts, &config[dollar_index..end_index + 1]);
            }
            index = end_index + 1;
            continue;
        }

        if let Some(name) = env_var_name_prefix(&config[dollar_index + 1..]) {
            parts.push(TemplatePart::Env(name.to_string()));
            index = dollar_index + 1 + name.len();
            continue;
        }

        append_literal(&mut parts, "$");
        index = dollar_index + 1;
    }

    parts
}

/// Port of `parseConfigValueReference` (:80-86).
fn parse_reference(config: &str) -> ConfigValueReference {
    if config.starts_with('!') {
        ConfigValueReference::Command(config.to_string())
    } else {
        ConfigValueReference::Template(parse_template(config))
    }
}

/// Port of `resolveEnvConfigValue` (:88-90): provider-scoped `env` first, then process env;
/// empty strings are treated as unset (JS `||` falsiness).
fn resolve_env_value(name: &str, env: Option<&HashMap<String, String>>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name)
        && !v.is_empty()
    {
        return Some(v.clone());
    }
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

fn template_env_var_names(parts: &[TemplatePart]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in parts {
        if let TemplatePart::Env(name) = part
            && !names.iter().any(|n| n == name)
        {
            names.push(name.clone());
        }
    }
    names
}

/// Port of `resolveTemplate` (:101-113): any missing env var fails the whole template.
fn resolve_template(
    parts: &[TemplatePart],
    env: Option<&HashMap<String, String>>,
) -> Option<String> {
    let mut resolved = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(v) => resolved.push_str(v),
            TemplatePart::Env(name) => {
                let v = resolve_env_value(name, env)?;
                resolved.push_str(&v);
            }
        }
    }
    Some(resolved)
}

/// Port of `getConfigValueEnvVarName` (:115-119): the single env-var name iff the template is
/// exactly one `$VAR` reference.
pub fn config_value_env_var_name(config: &str) -> Option<String> {
    match parse_reference(config) {
        ConfigValueReference::Template(parts) => match parts.as_slice() {
            [TemplatePart::Env(name)] => Some(name.clone()),
            _ => None,
        },
        ConfigValueReference::Command(_) => None,
    }
}

/// Port of `getConfigValueEnvVarNames` (:121-124).
pub fn config_value_env_var_names(config: &str) -> Vec<String> {
    match parse_reference(config) {
        ConfigValueReference::Template(parts) => template_env_var_names(&parts),
        ConfigValueReference::Command(_) => Vec::new(),
    }
}

/// Port of `getMissingConfigValueEnvVarNames` (:126-128).
pub fn missing_config_value_env_var_names(
    config: &str,
    env: Option<&HashMap<String, String>>,
) -> Vec<String> {
    config_value_env_var_names(config)
        .into_iter()
        .filter(|name| resolve_env_value(name, env).is_none())
        .collect()
}

/// Port of `isCommandConfigValue` (:130-132).
pub fn is_command_config_value(config: &str) -> bool {
    matches!(parse_reference(config), ConfigValueReference::Command(_))
}

/// Port of `isConfigValueConfigured` (:134-136).
pub fn is_config_value_configured(config: &str, env: Option<&HashMap<String, String>>) -> bool {
    missing_config_value_env_var_names(config, env).is_empty()
}

/// Run `command` (without its leading `!`), returning trimmed stdout (Pi `executeCommandUncached`,
/// resolve-config-value.ts:198-206). On win32 Pi first tries the **configured shell**
/// (`executeWithConfiguredShell` → `getShellConfig`: Git Bash / PATH bash, with the legacy-WSL bash
/// stdin transport) and only falls back to the default shell when that shell is absent
/// (`executed === false`); on every other platform it uses the default shell directly
/// (`executeWithDefaultShell` = `execSync`, i.e. `/bin/sh -c`).
fn execute_shell(command: &str) -> Option<String> {
    if cfg!(windows) {
        let (executed, value) = execute_with_configured_shell(command);
        if executed {
            value
        } else {
            execute_with_default_shell(command)
        }
    } else {
        execute_with_default_shell(command)
    }
}

/// The default-shell command (Pi `executeWithDefaultShell` = `execSync`, :186-196): `cmd /C` on
/// Windows (via `%ComSpec%`), `sh -c` elsewhere.
fn default_shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// Port of `executeWithDefaultShell` (resolve-config-value.ts:186-196): run via the default shell;
/// a non-zero exit, a spawn failure, empty output, or a timeout all yield `None` (Pi's `execSync`
/// throws on non-zero → `catch` → `undefined`).
fn execute_with_default_shell(command: &str) -> Option<String> {
    match run_with_timeout(default_shell_command(command), None) {
        Ok((true, value)) => value,
        _ => None,
    }
}

/// Port of `executeWithConfiguredShell` (resolve-config-value.ts:153-184). Resolves the configured
/// shell (`getShellConfig`) and runs `command` through it. The returned `executed` flag mirrors Pi:
/// `false` ⇒ the shell could not be located (ENOENT, or `getShellConfig` threw) so the caller must
/// fall back to the default shell; `true` ⇒ the shell ran (the value is `None` on a non-zero exit
/// or other spawn error).
fn execute_with_configured_shell(command: &str) -> (bool, Option<String>) {
    // `getShellConfig` throwing (no bash on win32) is caught by Pi and reported as not-executed.
    let Some(config) = get_shell_config() else {
        return (false, None);
    };
    let mut cmd = Command::new(&config.shell);
    cmd.args(&config.args);
    let stdin_input = if config.command_from_stdin {
        Some(command)
    } else {
        cmd.arg(command);
        None
    };
    match run_with_timeout(cmd, stdin_input) {
        Ok((true, value)) => (true, value),
        // Ran but exited non-zero → executed, no value.
        Ok((false, _)) => (true, None),
        // ENOENT (shell binary vanished) → not executed; fall back to the default shell.
        Err(e) if e.kind() == ErrorKind::NotFound => (false, None),
        // Any other spawn error → executed, no value (Pi: `result.error` non-ENOENT).
        Err(_) => (true, None),
    }
}

/// Spawn `cmd` (stdout piped, stderr ignored), optionally feeding `stdin_input` on stdin, enforcing
/// the same 10s timeout Pi passes to `spawnSync`/`execSync`. `Err` ⇒ the process could not be
/// spawned (the `io::ErrorKind` distinguishes ENOENT); `Ok((success, value))` ⇒ it ran, with
/// `success` the zero-exit flag and `value` the trimmed stdout (`None` when empty).
fn run_with_timeout(
    mut cmd: Command,
    stdin_input: Option<&str>,
) -> Result<(bool, Option<String>), std::io::Error> {
    cmd.stdin(if stdin_input.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    })
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());

    let mut child = cmd.spawn()?;

    if let Some(input) = stdin_input
        && let Some(mut sink) = child.stdin.take()
    {
        use std::io::Write as _;
        let _ = sink.write_all(input.as_bytes());
        // Drop `sink` to send EOF so the shell can run the piped command.
    }

    let start = Instant::now();
    let timeout = Duration::from_millis(10_000);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output()?;
                let text = String::from_utf8_lossy(&output.stdout);
                let trimmed = text.trim();
                let value = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
                return Ok((status.success(), value));
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok((false, None));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Resolved shell invocation (Pi `ShellConfig`, utils/shell.ts:6-10): the shell binary, its argv,
/// and whether the command is delivered on stdin (`commandTransport === "stdin"`, legacy WSL bash)
/// rather than as a trailing argv entry.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ShellConfig {
    shell: String,
    args: Vec<String>,
    command_from_stdin: bool,
}

/// Port of `isLegacyWslBashPath` (utils/shell.ts:14-17): the Windows-shipped WSL bash shim
/// (`<drive>:\Windows\System32\bash.exe` or `…\Sysnative\bash.exe`), which only accepts a command
/// via stdin (`bash -s`). Matches `^[a-z]:\windows\(system32|sysnative)\bash\.exe$` on the
/// backslash-normalized, lowercased path.
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_ascii_lowercase();
    let Some((drive, rest)) = normalized.split_once(':') else {
        return false;
    };
    let mut drive_chars = drive.chars();
    let (Some(c), None) = (drive_chars.next(), drive_chars.next()) else {
        return false;
    };
    if !c.is_ascii_lowercase() {
        return false;
    }
    matches!(
        rest,
        "\\windows\\system32\\bash.exe" | "\\windows\\sysnative\\bash.exe"
    )
}

/// Port of `getBashShellConfig` (utils/shell.ts:19-21): legacy WSL bash takes the command on stdin
/// (`-s`); every other bash takes it as a `-c` argument.
fn bash_shell_config(shell: &str) -> ShellConfig {
    if is_legacy_wsl_bash_path(shell) {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-s".to_string()],
            command_from_stdin: true,
        }
    } else {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-c".to_string()],
            command_from_stdin: false,
        }
    }
}

/// Port of `findBashOnPath` (utils/shell.ts:23-58): locate `bash` via `where bash.exe` (win32, with
/// an existence check since `where` can report stale paths) or `which bash` (unix, trusted). Pi
/// caps the lookup at 5s; `Command::output` blocks, but `where`/`which` return promptly.
fn find_bash_on_path() -> Option<String> {
    let (program, arg) = if cfg!(windows) {
        ("where", "bash.exe")
    } else {
        ("which", "bash")
    };
    let output = Command::new(program).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    if cfg!(windows) && !Path::new(first).exists() {
        return None;
    }
    Some(first.to_string())
}

/// Port of `getShellConfig()` called with no `customShellPath` (utils/shell.ts:65-122, the form
/// `resolveConfigValue` uses at :155). Resolution order — win32: Git Bash in
/// `%ProgramFiles%`/`%ProgramFiles(x86)%`, then `bash.exe` on PATH, else `None` (Pi throws, which
/// the caller treats as not-executed); unix: `/bin/bash`, then `bash` on PATH, then `sh -c`.
fn get_shell_config() -> Option<ShellConfig> {
    if cfg!(windows) {
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(format!("{program_files}\\Git\\bin\\bash.exe"));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(format!("{program_files_x86}\\Git\\bin\\bash.exe"));
        }
        for path in &candidates {
            if Path::new(path).exists() {
                return Some(bash_shell_config(path));
            }
        }
        find_bash_on_path().map(|bash| bash_shell_config(&bash))
    } else {
        if Path::new("/bin/bash").exists() {
            return Some(bash_shell_config("/bin/bash"));
        }
        if let Some(bash) = find_bash_on_path() {
            return Some(bash_shell_config(&bash));
        }
        Some(ShellConfig {
            shell: "sh".to_string(),
            args: vec!["-c".to_string()],
            command_from_stdin: false,
        })
    }
}

/// Port of `executeCommandUncached` (:198-206): strip the leading `!` and run.
fn execute_command_uncached(command_config: &str) -> Option<String> {
    let command = command_config.get(1..).unwrap_or("");
    execute_shell(command)
}

/// Port of `executeCommand` (:208-216): cached for the process lifetime.
fn execute_command(command_config: &str) -> Option<String> {
    if let Ok(cache) = command_cache().lock()
        && let Some(cached) = cache.get(command_config)
    {
        return cached.clone();
    }
    let result = execute_command_uncached(command_config);
    if let Ok(mut cache) = command_cache().lock() {
        cache.insert(command_config.to_string(), result.clone());
    }
    result
}

/// Resolve a config value to an actual value (Pi `resolveConfigValue`, :145-151). `!command`
/// results are cached; templates interpolate `env` then the process environment.
pub fn resolve_config_value(config: &str, env: Option<&HashMap<String, String>>) -> Option<String> {
    match parse_reference(config) {
        ConfigValueReference::Command(c) => execute_command(&c),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

/// Like [`resolve_config_value`] but bypasses the command cache (Pi `resolveConfigValueUncached`,
/// :221-227) — used for header resolution where a fresh value is wanted.
pub fn resolve_config_value_uncached(
    config: &str,
    env: Option<&HashMap<String, String>>,
) -> Option<String> {
    match parse_reference(config) {
        ConfigValueReference::Command(c) => execute_command_uncached(&c),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

/// Port of `resolveConfigValueOrThrow` (:229-251): resolve uncached or return a descriptive error.
pub fn resolve_config_value_or_throw(
    config: &str,
    description: &str,
    env: Option<&HashMap<String, String>>,
) -> Result<String, String> {
    if let Some(v) = resolve_config_value_uncached(config, env) {
        return Ok(v);
    }
    match parse_reference(config) {
        ConfigValueReference::Command(c) => Err(format!(
            "Failed to resolve {description} from shell command: {}",
            c.get(1..).unwrap_or("")
        )),
        ConfigValueReference::Template(_) => {
            let missing = missing_config_value_env_var_names(config, env);
            match missing.len() {
                1 => Err(format!(
                    "Failed to resolve {description} from environment variable: {}",
                    missing.first().map(String::as_str).unwrap_or("")
                )),
                n if n > 1 => Err(format!(
                    "Failed to resolve {description} from environment variables: {}",
                    missing.join(", ")
                )),
                _ => Err(format!("Failed to resolve {description}")),
            }
        }
    }
}

/// Async entry point for [`resolve_config_value`], for callers already inside a tokio runtime.
///
/// The body is unchanged and synchronous — pi's is too (`execSync(command, { timeout: 10000 })`,
/// resolve-config-value.ts:186-196 @v0.83.0, inside an async `resolve`) — but a `!command`
/// credential helper can occupy the calling thread for up to the full 10 s ceiling. pi has one
/// event loop to block; cyrup has N tokio workers, and holding one of them for 10 s degrades
/// unrelated concurrent work. Moving the blocking body onto the blocking pool keeps pi's timing
/// EXACTLY (the 10 s is pi's number and is not touched here) while freeing the worker. CFG-028.
pub async fn resolve_config_value_async(
    config: &str,
    env: Option<&HashMap<String, String>>,
) -> Option<String> {
    // A pure template needs no process at all; staying on the caller's thread avoids a pool
    // round-trip on the overwhelmingly common path.
    if !is_command_config_value(config) {
        return resolve_config_value(config, env);
    }
    let config = config.to_string();
    let env = env.cloned();
    // A panic in the blocking body is "unresolvable", the same answer the sync path gives for
    // a command that fails.
    tokio::task::spawn_blocking(move || resolve_config_value(&config, env.as_ref()))
        .await
        .unwrap_or_default()
}

/// Async entry point for [`resolve_config_value_or_throw`]. See
/// [`resolve_config_value_async`] for why the blocking body is moved off the worker. CFG-028.
pub async fn resolve_config_value_or_throw_async(
    config: &str,
    description: &str,
    env: Option<&HashMap<String, String>>,
) -> Result<String, String> {
    if !is_command_config_value(config) {
        return resolve_config_value_or_throw(config, description, env);
    }
    let config_owned = config.to_string();
    let description_owned = description.to_string();
    let env_owned = env.cloned();
    match tokio::task::spawn_blocking(move || {
        resolve_config_value_or_throw(&config_owned, &description_owned, env_owned.as_ref())
    })
    .await
    {
        Ok(v) => v,
        Err(_) => Err(format!("Failed to resolve {description}")),
    }
}

/// Port of `resolveHeaders` (:256-269): resolve each header value (cached); drop ones that resolve
/// to nothing; `None` when no header survives./// Port of `resolveHeaders` (:256-269): resolve each header value (cached); drop ones that resolve
/// to nothing; `None` when no header survives.
pub fn resolve_headers(
    headers: Option<&HashMap<String, String>>,
    env: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let headers = headers?;
    let mut resolved = HashMap::new();
    for (key, value) in headers {
        if let Some(v) = resolve_config_value(value, env)
            && !v.is_empty()
        {
            resolved.insert(key.clone(), v);
        }
    }
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

/// Port of `resolveHeadersOrThrow` (:271-282).
pub fn resolve_headers_or_throw(
    headers: Option<&HashMap<String, String>>,
    description: &str,
    env: Option<&HashMap<String, String>>,
) -> Result<Option<HashMap<String, String>>, String> {
    let Some(headers) = headers else {
        return Ok(None);
    };
    let mut resolved = HashMap::new();
    for (key, value) in headers {
        let v =
            resolve_config_value_or_throw(value, &format!("{description} header \"{key}\""), env)?;
        resolved.insert(key.clone(), v);
    }
    Ok(if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    })
}

/// Clear the command cache (Pi `clearConfigValueCache`, :285-287). Exported for tests.
pub fn clear_config_value_cache() {
    if let Ok(mut cache) = command_cache().lock() {
        cache.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn literal_passthrough() {
        assert_eq!(
            resolve_config_value("sk-literal-key", None).as_deref(),
            Some("sk-literal-key")
        );
    }

    #[test]
    fn env_var_braced_and_bare() {
        let env = env_of(&[("ANTHROPIC_API_KEY", "sk-ant-from-map")]);
        assert_eq!(
            resolve_config_value("$ANTHROPIC_API_KEY", Some(&env)).as_deref(),
            Some("sk-ant-from-map")
        );
        assert_eq!(
            resolve_config_value("${ANTHROPIC_API_KEY}", Some(&env)).as_deref(),
            Some("sk-ant-from-map")
        );
        // prefix + var + suffix interpolation
        let env = env_of(&[("TOK", "abc")]);
        assert_eq!(
            resolve_config_value("Bearer ${TOK}!", Some(&env)).as_deref(),
            Some("Bearer abc!")
        );
    }

    #[test]
    fn missing_env_var_fails_whole_template() {
        // R: any missing env var means the whole value is unresolved.
        let env = env_of(&[]);
        assert_eq!(
            resolve_config_value("$DEFINITELY_UNSET_VAR_XYZ", Some(&env)),
            None
        );
        assert_eq!(
            missing_config_value_env_var_names("$DEFINITELY_UNSET_VAR_XYZ", Some(&env)).len(),
            1
        );
        assert!(!is_config_value_configured(
            "$DEFINITELY_UNSET_VAR_XYZ",
            Some(&env)
        ));
    }

    #[test]
    fn dollar_and_bang_escapes() {
        // `$$` -> `$`, `$!` -> `!` (resolve-config-value.ts:42-46).
        assert_eq!(resolve_config_value("a$$b", None).as_deref(), Some("a$b"));
        assert_eq!(resolve_config_value("a$!b", None).as_deref(), Some("a!b"));
    }

    #[test]
    fn unterminated_brace_is_literal_dollar() {
        // No closing `}` → literal `$` then the rest as literal (:50-53).
        assert_eq!(
            resolve_config_value("${UNCLOSED", None).as_deref(),
            Some("${UNCLOSED")
        );
    }

    #[test]
    fn invalid_brace_name_is_literal() {
        // `${1bad}` is not a valid env name → kept literal (:59-61).
        assert_eq!(
            resolve_config_value("${1bad}", None).as_deref(),
            Some("${1bad}")
        );
    }

    #[test]
    fn single_env_var_name_introspection() {
        assert_eq!(config_value_env_var_name("$FOO").as_deref(), Some("FOO"));
        assert_eq!(config_value_env_var_name("pre$FOO"), None);
        assert_eq!(config_value_env_var_name("!echo hi"), None);
        assert_eq!(
            config_value_env_var_names("$A-$B"),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn command_detection_and_execution() {
        clear_config_value_cache();
        assert!(is_command_config_value("!printf hello"));
        assert!(!is_command_config_value("$FOO"));
        #[cfg(unix)]
        {
            assert_eq!(
                resolve_config_value("!printf cmd-key", None).as_deref(),
                Some("cmd-key")
            );
            // cached: a second call returns the same value even though it would re-run.
            assert_eq!(
                resolve_config_value("!printf cmd-key", None).as_deref(),
                Some("cmd-key")
            );
            // non-zero exit → None
            assert_eq!(resolve_config_value("!false", None), None);
        }
    }

    #[test]
    fn or_throw_messages() {
        let env = env_of(&[]);
        let e = resolve_config_value_or_throw("$MISSING_ENV_VAR_ABC", "api key", Some(&env))
            .unwrap_err();
        assert!(
            e.contains("environment variable: MISSING_ENV_VAR_ABC"),
            "{e}"
        );
    }

    #[test]
    fn headers_resolution_drops_unresolved() {
        let env = env_of(&[("H1", "v1")]);
        let mut headers = HashMap::new();
        headers.insert("X-One".to_string(), "$H1".to_string());
        headers.insert("X-Two".to_string(), "$MISSING_HEADER_VAR".to_string());
        let resolved = resolve_headers(Some(&headers), Some(&env)).unwrap();
        assert_eq!(resolved.get("X-One").map(String::as_str), Some("v1"));
        assert!(!resolved.contains_key("X-Two"));
    }

    #[test]
    fn legacy_wsl_bash_path_detection() {
        // Pi isLegacyWslBashPath (utils/shell.ts:14-17): drive:\Windows\System32|Sysnative\bash.exe.
        assert!(is_legacy_wsl_bash_path(r"C:\Windows\System32\bash.exe"));
        assert!(is_legacy_wsl_bash_path(r"c:\windows\sysnative\bash.exe"));
        // forward slashes are normalized to backslashes first.
        assert!(is_legacy_wsl_bash_path("D:/Windows/System32/bash.exe"));
        // Git Bash / other locations are NOT the legacy WSL shim.
        assert!(!is_legacy_wsl_bash_path(
            r"C:\Program Files\Git\bin\bash.exe"
        ));
        assert!(!is_legacy_wsl_bash_path(r"C:\Windows\System32\cmd.exe"));
        assert!(!is_legacy_wsl_bash_path("/bin/bash"));
        assert!(!is_legacy_wsl_bash_path("bash"));
    }

    #[test]
    fn bash_shell_config_picks_transport() {
        // Pi getBashShellConfig (utils/shell.ts:19-21): legacy WSL bash → stdin (`-s`); else `-c`.
        let legacy = bash_shell_config(r"C:\Windows\System32\bash.exe");
        assert!(legacy.command_from_stdin);
        assert_eq!(legacy.args, vec!["-s".to_string()]);

        let normal = bash_shell_config("/bin/bash");
        assert!(!normal.command_from_stdin);
        assert_eq!(normal.args, vec!["-c".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn get_shell_config_unix_resolves_a_command_shell() {
        // Pi getShellConfig unix branch (utils/shell.ts:111-121): /bin/bash, then PATH bash, then sh.
        let cfg = get_shell_config().expect("unix always resolves a shell");
        assert!(!cfg.shell.is_empty());
        assert!(!cfg.command_from_stdin); // no legacy-WSL transport off-Windows
        assert_eq!(cfg.args, vec!["-c".to_string()]);
        // The resolved shell actually executes a command end-to-end.
        let (executed, value) = execute_with_configured_shell("printf cfg-shell");
        assert!(executed);
        assert_eq!(value.as_deref(), Some("cfg-shell"));
        // Non-zero exit → executed, but no value.
        let (executed, value) = execute_with_configured_shell("exit 3");
        assert!(executed);
        assert_eq!(value, None);
    }
}
