//! The live-fetch half of `gen-catalogs` (XAI_1).
//!
//! `CATALOGS` in `main.rs` recovers 34 catalogs from a pinned pi revision with `git show`. That
//! mechanism is DEAD for xai: pi's `xai.models.ts` is a two-line re-export of a gitignored,
//! network-generated JSON file from `a9f6a3159` onward, so no revision carries the rows. pi
//! publishes the already-shaped rows at `https://pi.dev/api/models/providers/xai` instead — the
//! SAME endpoint `cyrup-provider/src/remote_catalog.rs` overlays at runtime — so this module takes
//! them from there.
//!
//! DEPENDENCY-FREE, like the rest of this crate (`xtask/Cargo.toml`): HTTP is `curl` shelled out
//! exactly as `git_show` shells out `git`, and JSON is `tsdata`'s own order-preserving reader and
//! writer. Round-tripping through `cyrup_provider::Model` is deliberately NOT done — it would add
//! the whole provider stack to a build-time tool's compile path, and `parse_catalog`'s
//! `filter_map(.ok())` SILENTLY DROPS rows it cannot deserialize, which is the one thing this
//! generator is not allowed to do.

use crate::tsdata::{self, Val};
use std::process::Command;

/// One catalog whose rows are fetched live instead of recovered from the pinned revision.
#[derive(Debug)]
pub struct LiveCatalogSpec {
    /// `providers/catalog/<file>.json`.
    pub file: &'static str,
    /// The provider id every row must carry.
    pub provider: &'static str,
    /// The endpoint that serves the rows, already shaped into cyrup's native `Model` JSON.
    pub url: &'static str,
    /// The upstream module the rows STILL originate from, kept for the manifest's `module` field:
    /// it is a re-export of gitignored data now, but it is what pi ships and what a future
    /// un-blocking would read.
    pub module: &'static str,
    /// The gap-analysis item that authorised the live path.
    pub item: &'static str,
}

/// The response, reduced to what the generator needs.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub body: String,
    /// `last-modified`, verbatim (RFC 1123).
    pub last_modified: Option<String>,
    /// `x-pi-model-catalog-revision` — pi's content hash, the live counterpart of a commit sha.
    pub revision: Option<String>,
    /// Captured for parity with the runtime overlay's own fetch (`remote_catalog.rs`), which DOES
    /// revalidate with it; this generator always does a fresh unconditional GET (D2's "one write
    /// pass" needs the current body every run), so it has no validator to send and never reads
    /// this back. Kept on the struct rather than dropped so a future revalidating mode does not
    /// have to re-thread it through `fetch_with_curl`'s header parsing.
    #[allow(dead_code)]
    pub etag: Option<String>,
}

/// What one live catalog contributed to this run.
#[derive(Debug)]
pub enum LiveOutcome {
    /// Rows arrived: `body` is the file to write, `fetched_at`/`revision` are its provenance.
    Fetched {
        body: String,
        fetched_at: Option<String>,
        revision: Option<String>,
    },
    /// The fetch could not happen. `xai.json` must be left exactly as it is and the previous
    /// manifest entry carried forward (D6). `why` is printed as a notice, never as an error.
    Skipped { why: String },
}

/// Fetch `url` with `curl`, mirroring `main.rs::git_show`'s shell-out.
///
/// `--fail` folds every HTTP >= 400 into a non-zero exit, `--max-time` bounds a hung origin, and
/// `--dump-header` is how the provenance headers are recovered without an HTTP library. Every
/// failure here is a NON-FATAL skip (D3): a maintainer on a plane must still be able to regenerate
/// the other 34 catalogs.
pub fn fetch_with_curl(url: &str) -> Result<Fetched, String> {
    let header_path =
        std::env::temp_dir().join(format!("xtask-live-headers-{}", std::process::id()));
    let out = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "20",
        ])
        .arg("--dump-header")
        .arg(&header_path)
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    let headers = std::fs::read_to_string(&header_path).unwrap_or_default();
    let _ = std::fs::remove_file(&header_path);
    if !out.status.success() {
        return Err(format!(
            "curl {url} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let body = String::from_utf8(out.stdout).map_err(|e| format!("{url} is not UTF-8: {e}"))?;
    Ok(Fetched {
        last_modified: header_value(&headers, "last-modified"),
        revision: header_value(&headers, "x-pi-model-catalog-revision"),
        etag: header_value(&headers, "etag"),
        body,
    })
}

/// Case-insensitive header lookup over a raw `--dump-header` dump. Takes the LAST occurrence so a
/// `--location` redirect chain yields the final response's value.
fn header_value(dump: &str, name: &str) -> Option<String> {
    dump.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
        .rfind(|v| !v.is_empty())
}

/// The rows of one live catalog, validated. HARD ERROR on anything unexpected (D3): the body
/// arrived, so a shape we do not understand is a contract change, not an outage.
pub fn rows_from_body(spec: &LiveCatalogSpec, body: &str) -> Result<Vec<Val>, String> {
    let doc = tsdata::parse_json(body).map_err(|e| format!("{}: {e}", spec.url))?;
    // The endpoint binds `id -> Model`, the same shape `remote_catalog::parse_catalog` accepts.
    let rows = tsdata::object_values(&doc)
        .map_err(|e| format!("{}: expected an id-keyed object of models: {e}", spec.url))?;
    if rows.is_empty() {
        return Err(format!(
            "{}: returned zero rows — refusing to write an empty catalog over {}.json",
            spec.url, spec.file
        ));
    }
    for row in &rows {
        let id = row
            .get("id")
            .and_then(Val::as_str)
            .ok_or_else(|| format!("{}: a row has no string `id`", spec.url))?;
        // `parse_catalog` FORCES `provider`; a build-time generator must instead refuse, because
        // `providers/fleet.rs` asserts every row is tagged with its own provider id and a silent
        // rewrite would hide an endpoint that started serving somebody else's rows.
        match row.get("provider").and_then(Val::as_str) {
            Some(p) if p == spec.provider => {}
            other => {
                return Err(format!(
                    "{}: row `{id}` is tagged provider {other:?}, expected {:?}",
                    spec.url, spec.provider
                ));
            }
        }
    }
    Ok(rows)
}

/// The file body, in the exact byte shape every other catalog already uses.
pub fn render(rows: Vec<Val>) -> String {
    let mut body = Val::Arr(rows).to_json();
    body.push('\n');
    body
}

/// `Fri, 11 Sep 2026 10:28:26 GMT` -> `2026-09-11T10:28:26Z`.
///
/// The manifest's other timestamps are ISO-8601 and `cyrup-provider`'s `parse_iso8601_utc_ms` is
/// what consumes them (`providers/all.rs`), so the live stamp has to arrive in the same shape.
/// Always GMT, so this is a pure reformat with no timezone math.
pub fn http_date_to_iso8601(value: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = value.split_whitespace();
    let _weekday = parts.next()?;
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    let time = parts.next()?;
    if parts.next() != Some("GMT") || day.len() > 2 || year.len() != 4 || time.len() != 8 {
        return None;
    }
    let month = MONTHS.iter().position(|m| *m == month)? + 1;
    Some(format!("{year}-{month:02}-{day:0>2}T{time}Z"))
}

/// Fetch every live catalog, or say cleanly why not.
///
/// `fetch` is the only transport seam (D8): production passes [`fetch_with_curl`], anything that
/// needs determinism passes a closure over a fixture body. Nothing here touches the filesystem —
/// the caller owns writing, so `--check` and `--diff` get the live rows for free.
///
/// `CYRUP_XTASK_SKIP_LIVE=1` forces every spec to [`LiveOutcome::Skipped`] without invoking
/// `fetch` at all — the escape hatch D3 promises a maintainer with no network.
pub fn refresh(
    specs: &'static [LiveCatalogSpec],
    fetch: &dyn Fn(&str) -> Result<Fetched, String>,
) -> Result<Vec<(&'static LiveCatalogSpec, LiveOutcome)>, String> {
    let skip_all = std::env::var_os("CYRUP_XTASK_SKIP_LIVE").is_some();
    let mut out = Vec::new();
    for spec in specs {
        if skip_all {
            out.push((
                spec,
                LiveOutcome::Skipped {
                    why: "CYRUP_XTASK_SKIP_LIVE is set".to_string(),
                },
            ));
            continue;
        }
        match fetch(spec.url) {
            Err(why) => out.push((spec, LiveOutcome::Skipped { why })),
            Ok(res) => {
                let rows = rows_from_body(spec, &res.body)?; // hard error (D3)
                out.push((
                    spec,
                    LiveOutcome::Fetched {
                        body: render(rows),
                        fetched_at: res.last_modified.as_deref().and_then(http_date_to_iso8601),
                        revision: res.revision,
                    },
                ));
            }
        }
    }
    Ok(out)
}
