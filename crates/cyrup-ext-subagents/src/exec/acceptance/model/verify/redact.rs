//! G80 verify-command secret redaction (pi `acceptance.ts:974-994`): which environment keys
//! count as sensitive, and how they are masked before a command is recorded.

// --------------------------------------------------------------------------------------------
// G80: verify-command secret redaction (acceptance.ts:974-994)
// --------------------------------------------------------------------------------------------

/// The alternation inside upstream's `SENSITIVE_ENV_KEY_PATTERN`
/// (`acceptance.ts:974` @v0.43.0):
///
/// ```text
/// /(?:^|_)(?:TOKEN|SECRET|PASSWORD|PASS|AUTH|CREDENTIAL|COOKIE|SESSION|PRIVATE|API_KEY|ACCESS_KEY)(?:_|$)/i
/// ```
///
/// Copied VERBATIM and in upstream's order. This list is a security boundary — a verify
/// command's captured stdout/stderr goes straight into the acceptance ledger and from there
/// into a transcript, so anything this list misses is a credential that leaks. Do not "improve"
/// it locally; change it only to track upstream.
const SENSITIVE_ENV_KEY_WORDS: [&str; 11] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASS",
    "AUTH",
    "CREDENTIAL",
    "COOKIE",
    "SESSION",
    "PRIVATE",
    "API_KEY",
    "ACCESS_KEY",
];

/// `SENSITIVE_ENV_KEY_PATTERN.test(key)` (`acceptance.ts:974,985`), re-expressed as a scan so
/// the crate needs no regex dependency.
///
/// The pattern is unanchored and case-insensitive, so it matches when ANY word in
/// [`SENSITIVE_ENV_KEY_WORDS`] occurs at a `_`-or-boundary-delimited position anywhere in the
/// key: `GITHUB_TOKEN` and `TOKEN_FILE` and `AWS_SECRET_ACCESS_KEY` all match, while
/// `TOKENIZER` and `PASSAGE` do not (`I`/`A` is neither `_` nor end-of-string).
///
/// `to_ascii_uppercase` is what makes the `i` flag faithful without changing byte offsets —
/// env key names are ASCII, and a non-ASCII byte is left alone and simply never matches.
#[must_use]
fn is_sensitive_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    for word in SENSITIVE_ENV_KEY_WORDS {
        let needle = word.as_bytes();
        if needle.len() > bytes.len() {
            continue;
        }
        for start in 0..=(bytes.len() - needle.len()) {
            let end = start + needle.len();
            if bytes.get(start..end) != Some(needle) {
                continue;
            }
            // `(?:^|_)` before, `(?:_|$)` after.
            let left = start == 0 || bytes.get(start - 1) == Some(&b'_');
            let right = end == bytes.len() || bytes.get(end) == Some(&b'_');
            if left && right {
                return true;
            }
        }
    }
    false
}

/// `effectiveVerifyEnv` (`acceptance.ts:976-981`): `{ ...process.env, ...(env ?? {}) }` — the
/// command's declared pairs layered OVER the inherited environment, never replacing it.
///
/// Upstream's `flatMap` drops any `process.env` entry whose value is not a string; the Rust
/// analog is dropping any `vars_os` pair that is not valid UTF-8 (which is also why this reads
/// `vars_os` rather than `vars`, whose iterator panics on exactly that input — the no-panic
/// policy forbids it).
#[must_use]
pub(crate) fn effective_verify_env(
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> std::collections::BTreeMap<String, String> {
    let mut merged: std::collections::BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| {
            Some((key.into_string().ok()?, value.into_string().ok()?))
        })
        .collect();
    if let Some(declared) = env {
        for (key, value) in declared {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

/// `verifyRedactionEnv` (`acceptance.ts:983-987`): the effective environment filtered down to
/// the entries whose KEY looks sensitive and whose VALUE is at least 4 long — the length floor
/// upstream applies so that a short/degenerate value (`"1"`, `"on"`) cannot blanket-redact
/// every occurrence of that substring in otherwise-innocent output.
///
/// JS `.length` counts UTF-16 units where Rust `.len()` counts bytes; the two agree exactly for
/// the ASCII every real credential is made of, and both are monotone in string size, so the
/// longest-first ordering below is preserved either way.
#[must_use]
fn verify_redaction_env(
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> Vec<String> {
    effective_verify_env(env)
        .into_iter()
        .filter(|(key, value)| value.len() >= 4 && is_sensitive_env_key(key))
        .map(|(_, value)| value)
        .collect()
}

/// `redactVerifyEnv` (`acceptance.ts:989-994`): replace every occurrence of every sensitive
/// environment VALUE in `value` with `[REDACTED]`.
///
/// The de-duplicated secret list is sorted LONGEST FIRST (upstream
/// `.sort((left, right) => right.length - left.length)`), which is load-bearing: when one
/// secret is a prefix of another, redacting the short one first would leave the remainder of
/// the long one in the output. `str::replace` is a literal replacement, exactly like
/// `String.prototype.replaceAll` with a string (not regex) pattern.
#[must_use]
pub fn redact_verify_env(
    value: &str,
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> String {
    let mut secrets = verify_redaction_env(env);
    // `[...new Set(...)]` — dedupe. Sorting first makes `dedup` total, and the subsequent
    // stable length sort then leaves equal-length secrets in a deterministic order.
    secrets.sort();
    secrets.dedup();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    let mut redacted = value.to_string();
    for secret in secrets {
        // `.filter(Boolean)` (`acceptance.ts:991`) — an empty secret would otherwise splice
        // `[REDACTED]` between every character.
        if secret.is_empty() {
            continue;
        }
        redacted = redacted.replace(&secret, "[REDACTED]");
    }
    redacted
}
