//! The tmux keyboard-setup diagnostic — the port of Pi's `checkTmuxKeyboardSetup`
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:940-988`, wired at
//! `:865-869`).
//!
//! # The failure it names
//!
//! Inside tmux, a modified `Enter` (Shift+Enter, Ctrl+Enter — cyrup's newline-vs-submit distinction)
//! only reaches the application if tmux is forwarding extended keys, and only in a form the
//! application can parse:
//!
//! * `extended-keys` must be `on` or `always`; the default is `off`, and with it off tmux collapses
//!   every modified `Enter` to a plain `\r`. The keystroke is not "swallowed" — it arrives as a
//!   SUBMIT, which is worse than nothing: the user's newline posts their half-written prompt.
//! * `extended-keys-format` must be `csi-u`. Set to `xterm`, tmux emits xterm's
//!   `CSI 27 ; <mod> ; <code> ~` form instead of `CSI <code> ; <mod> u` — and crossterm's
//!   `parse_csi_special_key_code` rejects the `27` parameter outright
//!   (`crossterm-0.29.0/src/event/sys/unix/parse.rs:619-657`), so under cyrup the modified key is
//!   dropped rather than merely misread. This is the same decoder limit that keeps
//!   [`crate::keyboard_protocol`] from writing xterm's `modifyOtherKeys` fallback, which is why Pi
//!   flags the two together.
//!
//! Neither is discoverable from inside the session: everything else works, and only one key
//! misbehaves. Hence a startup warning rather than a fix — the setting lives in the user's
//! `~/.tmux.conf` and cannot be changed from here.
//!
//! # Shape of the port
//!
//! [`keyboard_warning`] is Pi's decision (`:973-987`) as a pure function over the two option values;
//! [`check_keyboard_setup`] is Pi's `runTmuxShow` pair (`:943-971`) — `tmux show -gv <option>`,
//! stdout captured, stderr and stdin closed, both queries issued concurrently (Pi's `Promise.all`),
//! each bounded by [`TMUX_QUERY_TIMEOUT`] with the child killed on expiry. Every failure mode —
//! no tmux binary, a non-zero exit, a timeout, a sandbox that blocks the spawn — resolves to "no
//! answer", and no answer means NO warning (Pi `:979`: "If we couldn't query tmux … don't warn").

use std::time::Duration;

/// Pi's 2000 ms per-query budget (`interactive-mode.ts:951`). The check runs off the run loop, so
/// this delays nothing; it only bounds a wedged `tmux` client.
pub const TMUX_QUERY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Pi `:981` — verbatim but for the rebrand-free wording (this string names no product).
pub const EXTENDED_KEYS_OFF_WARNING: &str = "tmux extended-keys is off. Modified Enter keys may \
     not work. Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.";

/// Pi `:985`, with `Pi` → `cyrup` (Pi's own copy names its product here).
pub const EXTENDED_KEYS_FORMAT_WARNING: &str = "tmux extended-keys-format is xterm. cyrup works \
     best with csi-u. Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.";

/// Whether this process is running inside tmux — Pi's `if (!process.env.TMUX) return undefined`
/// (`:941`). An EMPTY `TMUX` is falsy in JavaScript, so it counts as "not in tmux" here too.
pub fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}

/// Pi's warning decision (`interactive-mode.ts:973-987`) over the two `tmux show -gv` answers.
///
/// * `extended_keys` `None` ⇒ the query failed (timeout, no tmux, sandbox) ⇒ **no warning at all**,
///   including no format warning — Pi returns early at `:979` before ever looking at the format.
/// * `extended_keys` not `on`/`always` ⇒ [`EXTENDED_KEYS_OFF_WARNING`], and the format is not
///   reported: Pi returns the first warning it finds, one per startup.
/// * `extended_keys_format` exactly `xterm` ⇒ [`EXTENDED_KEYS_FORMAT_WARNING`].
pub fn keyboard_warning(
    extended_keys: Option<&str>,
    extended_keys_format: Option<&str>,
) -> Option<&'static str> {
    let extended_keys = extended_keys?;
    if extended_keys != "on" && extended_keys != "always" {
        return Some(EXTENDED_KEYS_OFF_WARNING);
    }
    if extended_keys_format == Some("xterm") {
        return Some(EXTENDED_KEYS_FORMAT_WARNING);
    }
    None
}

/// Run Pi's `checkTmuxKeyboardSetup` (`:940-988`) and return the warning to show, if any.
///
/// Spawned as its own task by [`crate::App::run`] and delivered to the transcript when it settles —
/// Pi does the same (`:865-869`: `this.checkTmuxKeyboardSetup().then(w => w && this.showWarning(w))`),
/// deliberately NOT awaited before the first frame.
pub async fn check_keyboard_setup() -> Option<&'static str> {
    if !in_tmux() {
        return None;
    }
    // Pi issues both `tmux show -gv` calls concurrently (`Promise.all`, `:970-973`).
    let (extended_keys, extended_keys_format) =
        tokio::join!(tmux_option("extended-keys"), tmux_option("extended-keys-format"));
    keyboard_warning(extended_keys.as_deref(), extended_keys_format.as_deref())
}

/// One `tmux show -gv <option>` — Pi's `runTmuxShow` (`:943-971`). `None` on a spawn failure, a
/// non-zero exit, or the [`TMUX_QUERY_TIMEOUT`] expiring; the value is trimmed like Pi's
/// `stdout.trim()`.
async fn tmux_option(option: &str) -> Option<String> {
    let child = tokio::process::Command::new("tmux")
        .args(["show", "-gv", option])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // Pi's timeout arm calls `proc.kill()`; dropping the future on timeout drops the child, and
        // `kill_on_drop` is what turns that drop into the same kill instead of a leaked process.
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let output = tokio::time::timeout(TMUX_QUERY_TIMEOUT, child.wait_with_output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;

    #[test]
    fn decision_table_matches_pis() {
        // `off` (tmux's default) and any other value ⇒ the extended-keys warning.
        assert_eq!(keyboard_warning(Some("off"), Some("csi-u")), Some(EXTENDED_KEYS_OFF_WARNING));
        assert_eq!(keyboard_warning(Some(""), None), Some(EXTENDED_KEYS_OFF_WARNING));
        // Both accepted values silence it (`extendedKeys !== "on" && … !== "always"`).
        assert_eq!(keyboard_warning(Some("on"), Some("csi-u")), None);
        assert_eq!(keyboard_warning(Some("always"), Some("csi-u")), None);
        // …and then the format is checked, but only for the exact string `xterm`.
        assert_eq!(
            keyboard_warning(Some("on"), Some("xterm")),
            Some(EXTENDED_KEYS_FORMAT_WARNING)
        );
        assert_eq!(keyboard_warning(Some("always"), None), None, "an unknown format is not xterm");
    }

    #[test]
    fn an_unanswerable_query_never_warns() {
        // Pi `:979`: a timeout / sandbox / missing binary must not produce a warning — a false
        // alarm on every non-tmux-aware environment would be worse than the missing diagnostic.
        assert_eq!(keyboard_warning(None, None), None);
        assert_eq!(keyboard_warning(None, Some("xterm")), None, "the format is not even reached");
    }

    /// Outside tmux the check must be a prompt no-op: it may not spawn anything, and it may not
    /// stall the task that awaits it. Skipped when the test harness itself runs inside tmux, where
    /// the answer legitimately depends on the user's `~/.tmux.conf`.
    #[tokio::test]
    async fn outside_tmux_the_check_is_an_immediate_no_op() {
        if in_tmux() {
            return;
        }
        let started = std::time::Instant::now();
        assert_eq!(check_keyboard_setup().await, None);
        assert!(started.elapsed() < TMUX_QUERY_TIMEOUT, "must short-circuit on $TMUX, not spawn");
    }
}
