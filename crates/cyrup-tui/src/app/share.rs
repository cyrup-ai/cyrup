/// Pi's project-trust banner, rebranded (`interactive-mode.ts:3506-3509` @v0.83.0). `${CONFIG_DIR_NAME}`
/// is `.cyrup` here — the directory [`cyrup_config::trust::has_trust_requiring_resources`] probes
/// (`trust.rs:211`) — and the closing `pi` is `cyrup`. TUI-N04.
/// The environment override for the `/share` viewer base URL — cyrup's rebranding of pi's
/// `PI_SHARE_VIEWER_URL` (`config.ts:506` @v0.83.0), and the name `cyrup --help` already advertises
/// at `crates/cyrup/src/cli.rs:1077` ("Base URL for /share command").
pub(crate) const ENV_SHARE_VIEWER_URL: &str = "CYRUP_SHARE_VIEWER_URL";

/// pi's `DEFAULT_SHARE_VIEWER_URL` (`config.ts:502` @v0.83.0), kept verbatim.
///
/// **Not a rebranding oversight.** The viewer is a pi-operated service that renders any GitHub gist
/// by id, so it works for a cyrup-produced gist unchanged, and this repo already points at that host
/// wherever the service is pi's — `cyrup-provider/src/remote_catalog.rs:68`
/// `DEFAULT_CATALOG_BASE_URL = "https://pi.dev"` and the referer headers at
/// `cyrup-session-svc/src/attribution.rs:82`. Substituting a cyrup host cyrup does not operate would
/// print a dead link on every `/share`.
pub(crate) const DEFAULT_SHARE_VIEWER_URL: &str = "https://pi.dev/session/";

/// pi's `const gistId = gistUrl?.split("/").pop();` (`interactive-mode.ts:5599` @v0.83.0) over the
/// URL `gh gist create` printed (`https://gist.github.com/<user>/<id>`).
///
/// JS `"abc".split("/")` is `["abc"]` and `pop()` returns `"abc"`, so a `gh` that printed a bare id
/// still resolves; only an empty tail (empty stdout, or a trailing `/`) is the failure pi's
/// `if (!gistId)` reports. `rsplit(..).next()` has exactly that shape — `"".rsplit('/').next()` is
/// `Some("")`, not `None`.
pub fn gist_id_from_url(gist_url: &str) -> &str {
    gist_url.rsplit('/').next().unwrap_or_default()
}

/// Port of pi's `getShareViewerUrl(gistId)` (`packages/coding-agent/src/config.ts:504-508`
/// @v0.83.0):
///
/// ```ts
/// export function getShareViewerUrl(gistId: string): string {
///     const baseUrl = process.env.PI_SHARE_VIEWER_URL || DEFAULT_SHARE_VIEWER_URL;
///     return `${baseUrl}#${gistId}`;
/// }
/// ```
///
/// JS `||` treats the empty string as unset, so an exported-but-empty variable falls back to the
/// default rather than producing a bare `#{id}` — hence the `filter(|v| !v.is_empty())`.
///
/// TUI-063: this is the ONLY consumer of [`ENV_SHARE_VIEWER_URL`]. Before it existed, `/share`
/// printed the raw gist URL and the advertised variable was inert.
pub fn share_viewer_url(gist_id: &str) -> String {
    share_viewer_url_from(std::env::var(ENV_SHARE_VIEWER_URL).ok().as_deref(), gist_id)
}

/// [`share_viewer_url`] with the environment already read — the same split
/// [`crate::status::experimental_features_enabled_from`] uses, so the `||` semantics are unit-testable
/// without `std::env::set_var` (`unsafe` in edition 2024, and this crate is `#![forbid(unsafe_code)]`).
#[must_use]
pub fn share_viewer_url_from(env_base: Option<&str>, gist_id: &str) -> String {
    let base = env_base.filter(|v| !v.is_empty()).unwrap_or(DEFAULT_SHARE_VIEWER_URL);
    format!("{base}#{gist_id}")
}

/// The in-flight half of a `/share` gist upload: the waiter task driving `gh gist create` and the
/// temp HTML file it is uploading.
///
/// pi keeps the same pair on the closure it installs as `loader.onAbort` — `proc` and `tmpFile`
/// (`session-share.ts:156-161`, `:76-84`) — because BOTH have to be reachable from the cancel path:
/// the child must be killed and the temp file must still be unlinked.
///
/// **Why an [`tokio::task::AbortHandle`] rather than the [`tokio::process::Child`] itself.**
/// `Child::wait_with_output` consumes the child, so the task that awaits `gh` owns it for the whole
/// upload and no second handle to it can exist. The command is therefore spawned with
/// `kill_on_drop(true)` and cancellation aborts the waiter: dropping the aborted task drops the
/// child, which kills `gh` — pi's `proc?.kill()` (`session-share.ts:158`) reached the only way a
/// consuming `await` allows.
pub(crate) struct ShareInFlight {
    /// The waiter task spawned by `App::share_session`; aborting it kills `gh` (see above).
    pub(crate) task: tokio::task::AbortHandle,
    /// The temp HTML file to unlink on the cancel path — pi's `finally` block (`:76-84`), which
    /// runs for a cancelled share exactly as it does for a successful one.
    pub(crate) tmp: std::path::PathBuf,
}

/// A settled `gh gist create --public=false <file>`, posted back to [`crate::app::App::run`]'s
/// `select!` so the upload never runs on the loop's own task — the channel-back shape
/// [`crate::app::TreeNavMsg`] established.
///
/// pi awaits the child inside a promise while its render loop keeps running
/// (`session-share.ts:172-186`); the equivalent here is a spawned task that owns
/// `wait_with_output()` and sends this message when it resolves.
#[derive(Debug)]
pub struct ShareMsg {
    /// `gh`'s exit status plus its captured streams, or the error `spawn`/`wait` failed with.
    pub(crate) result: Result<std::process::Output, std::io::Error>,
    /// The temp HTML file to unlink — pi's `finally { fs.unlinkSync(tmpFile) }` (`:76-84`).
    pub(crate) tmp: std::path::PathBuf,
}

impl ShareMsg {
    /// Pair a settled `gh` run with the temp file it uploaded. `pub` for the same reason
    /// [`crate::app::TreeNavMsg::new`] is: it lets `tests/*.rs` hand
    /// [`crate::app::App::apply_share_outcome`] a synthetic outcome — notably the cancelled and
    /// non-zero-exit cases, which are otherwise a race to provoke — without a live `gh`.
    pub fn new(
        result: Result<std::process::Output, std::io::Error>,
        tmp: std::path::PathBuf,
    ) -> Self {
        ShareMsg { result, tmp }
    }
}
