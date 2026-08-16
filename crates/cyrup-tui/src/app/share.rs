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

