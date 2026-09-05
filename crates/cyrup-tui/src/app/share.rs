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
    let base = env_base
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_SHARE_VIEWER_URL);
    format!("{base}#{gist_id}")
}

/// One entry of the tool list pi puts in the `pi.share` trailing entry
/// (`session-share.ts:35-39`). Borrowed rather than owned so the caller hands over its live
/// `ToolInfo` slice with no clone.
pub struct ShareTool<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a serde_json::Value,
}

/// pi `exportSessionForShare` (`session-share.ts:25-43` @v0.84.4), the export-only trailing entry
/// that makes a Radius share renderable:
///
/// ```ts
/// exportSessionToJsonl(session.sessionManager, filePath, (parentId, timestamp) => [{
///     type: "custom", customType: "pi.share", id: crypto.randomUUID().slice(0, 8),
///     parentId, timestamp,
///     data: { systemPrompt: session.state.systemPrompt,
///             tools: session.state.tools.map((t) => ({ name, description, parameters })) },
/// }]);
/// ```
///
/// `parentId` and `timestamp` are the two values `exportSessionToJsonl` computes and hands to the
/// callback (`core/session-export.ts:21`, `:31-36`): the timestamp is the one it stamped on the
/// SESSION HEADER, and `parentId` is the id of the last exported entry (`null` for an export with
/// no entries). Both are recovered here from the already-serialized JSONL rather than recomputed,
/// so the trailing entry cannot disagree with the document it is appended to.
///
/// `customType` stays `pi.share` unrebranded: it is a WIRE tag the Radius viewer matches on, in the
/// same class as [`DEFAULT_SHARE_VIEWER_URL`] above — renaming it would produce a share the service
/// cannot render.
///
/// `id` is the caller's (pi: `crypto.randomUUID().slice(0, 8)`), passed in so the line is a pure
/// function of its inputs and can be asserted byte for byte.
#[must_use]
pub fn append_share_metadata(
    jsonl: &str,
    id: &str,
    system_prompt: &str,
    tools: &[ShareTool<'_>],
) -> String {
    let mut lines = jsonl.lines().filter(|l| !l.trim().is_empty());
    let timestamp = lines
        .next()
        .and_then(|header| serde_json::from_str::<serde_json::Value>(header).ok())
        .and_then(|header| {
            header
                .get("timestamp")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    // `let parentId: string | null = null; for (…) { …; parentId = entry.id; }` — the last
    // exported entry's id, and `null` when the branch is empty.
    let parent_id = jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .skip(1)
        .last()
        .and_then(|last| serde_json::from_str::<serde_json::Value>(last).ok())
        .and_then(|last| {
            last.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    let entry = serde_json::json!({
        "type": "custom",
        "customType": "pi.share",
        "id": id,
        "parentId": parent_id,
        "timestamp": timestamp,
        "data": {
            "systemPrompt": system_prompt,
            "tools": tools
                .iter()
                .map(|t| serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }))
                .collect::<Vec<_>>(),
        },
    });
    // `writeFileSync(filePath, \`${lines.join("\n")}\n\`)` — every line terminated, including the
    // trailing one.
    let mut out = String::with_capacity(jsonl.len() + 256);
    out.push_str(jsonl.trim_end_matches('\n'));
    out.push('\n');
    out.push_str(&serde_json::to_string(&entry).unwrap_or_default());
    out.push('\n');
    out
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
    ///
    /// `None` on the Radius path (DRIFT-053): that upload POSTs the export from memory and writes
    /// no temp file, so there is nothing to unlink. pi writes one there
    /// (`session-share.ts:47`, `:52`) only because `exportSessionToJsonl` is a file writer.
    pub(crate) tmp: Option<std::path::PathBuf>,
    /// pi's `loader.signal` (`session-share.ts:123`) — the Radius upload's `AbortSignal`. `gh` is
    /// killed by dropping the aborted waiter (see above) and needs no token; an in-flight HTTP
    /// request is stopped by cancelling this one, which is what
    /// [`cyrup_provider::providers::radius_share::upload_share_artifact`] selects on.
    pub(crate) cancel: cyrup_core::CancelToken,
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
    /// Which of `/share`'s two upload paths settled, and what it settled to.
    pub(crate) upload: ShareUpload,
}

/// The settled result of ONE of `/share`'s two upload paths.
///
/// pi's `shareSession` is a two-step chain — `tryShareViaRadius` first, `shareViaGist` only when
/// that answers `false` (`session-share.ts:57`, `:77`) — and each step reports through its own
/// post-`await` tail with its own message wording. Modelling that as one enum rather than as a
/// `gh`-shaped `Output` plus a nullable Radius field is what keeps
/// [`crate::app::App::apply_share_outcome`] an exhaustive match: a third path added later cannot
/// compile until it says what the user is told.
#[derive(Debug)]
pub(crate) enum ShareUpload {
    /// `gh gist create --public=false <file>` settled — pi `shareViaGist` (`:152-203`).
    Gist {
        /// `gh`'s exit status plus its captured streams, or the error `spawn`/`wait` failed with.
        result: Result<std::process::Output, std::io::Error>,
        /// The temp HTML file to unlink — pi's `finally { fs.unlinkSync(tmpFile) }` (`:76-84`).
        tmp: std::path::PathBuf,
    },
    /// The Radius artifact `POST` settled — pi `tryShareViaRadius` (`:110-149`). DRIFT-053.
    /// `Err(Aborted)` is pi's `if (loader.signal.aborted) return true` (`:125`, `:130`): the
    /// cancel path has already printed `Share cancelled`, so this one prints nothing.
    Radius(Result<cyrup_provider::providers::radius_share::RadiusShareOutcome, String>),
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
        ShareMsg {
            upload: ShareUpload::Gist { result, tmp },
        }
    }

    /// The Radius sibling of [`ShareMsg::new`], `pub` for the same reason: a test drives the
    /// upload's three reported outcomes without a gateway.
    pub fn radius(
        outcome: Result<cyrup_provider::providers::radius_share::RadiusShareOutcome, String>,
    ) -> Self {
        ShareMsg {
            upload: ShareUpload::Radius(outcome),
        }
    }
}
