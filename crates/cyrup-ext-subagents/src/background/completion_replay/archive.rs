//! The companion archive: what a replayed completion can still point at (or quote) once its
//! payload is gone.

use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::background::wait_completions::non_empty;
use crate::exec::child_protocol::utf8_tail;

use super::{ARCHIVE_TEXT_LIMIT_BYTES, ARCHIVE_VERSION, completion_archive_path};

/// pi `CompletionArchiveEntry["source"]` (`completion-replay.ts:18`).
///
/// The three are ordered by fidelity, and [`write_completion_archive`] tries them in exactly that
/// order (`:76-103`): a saved artifact IS the real output; a session transcript can reconstruct it;
/// a bounded text tail is the last resort and the only one that can be truncated. An enum with an
/// exhaustive `match`, never a `String` — the set is closed by the on-disk format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveSource {
    /// `"output-artifact"` — `child.artifactPaths.outputPath`, verified to be an existing FILE.
    OutputArtifact,
    /// `"session"` — `child.sessionFile` (or the run-level one), verified likewise.
    Session,
    /// `"result-tail"` — bounded inline text; the only variant that can carry `truncated`.
    ResultTail,
}

/// pi `CompletionArchiveEntry` (`:15-22`). Every optional field is omit-when-absent so a pi
/// consumer reads a byte-identical object.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionArchiveEntry {
    /// The child's agent name, omitted when the child did not declare one (pi `:83`'s
    /// `...(agent ? { agent } : {})`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "tolerant_string"
    )]
    pub agent: Option<String>,
    /// The child's index within `results` — absent on the two RUN-level fallbacks, which belong to
    /// no child (`:106`, `:112`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "tolerant_result_index"
    )]
    pub result_index: Option<usize>,
    /// Which rung of the ladder produced this entry.
    pub source: ArchiveSource,
    /// The referenced file, for the two file-backed sources.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "tolerant_path"
    )]
    pub path: Option<PathBuf>,
    /// The retained text, for [`ArchiveSource::ResultTail`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "tolerant_string"
    )]
    pub text: Option<String>,
    /// `Some(true)` ONLY. pi writes `...(bounded.truncated ? { truncated: true } : {})` (`:100`),
    /// i.e. the key is absent rather than `false`; `skip_serializing_if` reproduces that, and
    /// [`tolerant_true`] reproduces `:179`'s `entry.truncated === true` on the read side.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "tolerant_true"
    )]
    pub truncated: Option<bool>,
}

/// pi `CompletionArchive` (`:24-29`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionArchive {
    /// pi `:25`. See [`ArchiveVersion`].
    pub version: ArchiveVersion,
    /// The run this archive belongs to.
    pub run_id: RunId,
    /// Epoch millis, the same `now` the record is written with (pi `:195`).
    pub created_at: i64,
    /// Per-entry tolerant on the read side — see [`tolerant_entries`].
    #[serde(deserialize_with = "tolerant_entries")]
    pub entries: Vec<CompletionArchiveEntry>,
}

/// The literal `1` of [`ARCHIVE_VERSION`], as a type — [`super::ReplayVersion`]'s sibling, and for
/// the same reason (pi `:168`'s `archive.version !== ARCHIVE_VERSION`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArchiveVersion;

impl serde::Serialize for ArchiveVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(ARCHIVE_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for ArchiveVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == ARCHIVE_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unknown completion archive version {raw}"
            )))
        }
    }
}

/// pi `parseArchive`'s per-entry `flatMap` (`:169-181`).
///
/// One malformed entry must DROP that entry, not fail the archive: a derived
/// `Vec<CompletionArchiveEntry>` is STRICTER than upstream and would turn a partially-corrupt
/// archive into a hard error, losing the good entries beside the bad one. A non-array `entries`
/// still fails the whole parse, exactly as `:168`'s `!Array.isArray` does.
fn tolerant_entries<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<CompletionArchiveEntry>, D::Error> {
    use serde::Deserialize as _;
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

/// pi `:174`/`:177`/`:178` — `typeof x === "string" ? { x } : {}`: a non-string coerces the FIELD
/// to absent rather than failing the entry.
fn tolerant_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    use serde::Deserialize as _;
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_str()
        .map(str::to_string))
}

/// [`tolerant_string`] for the file-backed sources' `path`.
fn tolerant_path<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<PathBuf>, D::Error> {
    Ok(tolerant_string(deserializer)?.map(PathBuf::from))
}

/// pi `:175` — a safe NON-NEGATIVE integer, or the field is absent. A negative, fractional or
/// non-numeric value drops the key and keeps the entry.
fn tolerant_result_index<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<usize>, D::Error> {
    use serde::Deserialize as _;
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_u64()
        .and_then(|index| usize::try_from(index).ok()))
}

/// pi `:179` — `entry.truncated === true`. Anything else, `false` included, omits the key.
fn tolerant_true<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    use serde::Deserialize as _;
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .filter(|flag| *flag))
}

/// pi `parseArchive` (`:165-183`) — tolerant by contract, like [`super::parse_replay`].
pub(super) fn parse_archive(bytes: &[u8]) -> Option<CompletionArchive> {
    serde_json::from_slice(bytes).ok()
}

/// pi `existingFile` (`:57-65`) — a non-empty string naming a path that is an existing FILE.
///
/// Not paranoia: an entry pointing at a path that is gone (or is a directory) would make a later
/// read FAIL rather than degrade, and the whole point of the archive is to be the thing that still
/// works.
async fn existing_file(value: Option<&serde_json::Value>) -> Option<PathBuf> {
    let path = PathBuf::from(non_empty(value)?);
    tokio::fs::metadata(&path)
        .await
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|_| path)
}

/// pi `outputArtifactPath` (`:67-70`) — `child.artifactPaths.outputPath`, but only when
/// `artifactPaths` is a non-array object.
async fn output_artifact_path(child: &serde_json::Value) -> Option<PathBuf> {
    let artifacts = child.get("artifactPaths")?;
    if !artifacts.is_object() {
        return None;
    }
    existing_file(artifacts.get("outputPath")).await
}

/// pi `writeCompletionArchive` (`:73-119`). Returns the archive's path.
///
/// # The ladder, per child (`:76-103`)
///
/// 1. `artifactPaths.outputPath` **that is an existing file** → reference it, no text copied;
/// 2. `sessionFile` **that is an existing file** → reference it;
/// 3. output/`error` present → retain a bounded TAIL, `Error: <error>` first then the output,
///    joined by `\n` (`:94`), truncated to [`ARCHIVE_TEXT_LIMIT_BYTES`].
///
/// A `results` element that is not a non-array object is skipped (`:78`), and a missing or
/// non-array `results` is the empty list (`:75`), which then triggers the run-level branch.
///
/// # rung 3 reads `finalOutput`, not upstream's `output` — deliberately
///
/// pi `:91` is `nonEmptyString(child.output)`, and **neither pi's `SingleResult` nor cyrup's
/// declares an `output` key**: pi's is `finalOutput?` (`shared/types.ts:1300`) and cyrup's
/// `final_output` serializes as `finalOutput` under `rename_all = "camelCase"`
/// (`exec/run_result.rs`). Ported verbatim, upstream's tail rung fires on `error` ALONE and is dead
/// for every successful child — precisely the text this archive exists to preserve. So the key that
/// actually carries a child's delivered text is read FIRST and upstream's spelling is kept as a
/// fallback, which is additive: both keys are read, the join order is unchanged, and a pi consumer
/// sees the same `{source:"result-tail", text, truncated?}` shape.
///
/// # The two run-level fallbacks
///
/// * `results.length === 0` → the RUN's own `sessionFile`, if it is a file (`:104-107`).
/// * still no entries → the run `summary`, bounded (`:108-114`). Neither `ResultFile` nor pi's
///   async payload declares a `summary`, so upstream's own fallback is likewise dead for a
///   cyrup-written payload — it is ported anyway because this projector's input is the same untyped
///   [`serde_json::Value`] a FOREIGN build may have written, and one carrying a `summary` should
///   still archive it.
///
/// An archive with zero entries is still written (`:115-118`): the record needs a companion file at
/// the canonical path or [`super::validate_replay_record`] would reject its own writer's output.
///
/// # Errors
///
/// A directory-creation, write, chmod or rename failure from
/// [`crate::background::atomic::write_private_atomic_json`].
pub async fn write_completion_archive(
    results_dir: &Path,
    run_id: &RunId,
    data: &serde_json::Value,
    created_at: i64,
) -> std::io::Result<PathBuf> {
    let mut entries: Vec<CompletionArchiveEntry> = Vec::new();
    let results = data
        .get("results")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    for (result_index, child) in results.iter().enumerate() {
        // pi `:78` — a non-object (or array) element is not a child.
        if !child.is_object() {
            continue;
        }
        let agent = non_empty(child.get("agent"));
        if let Some(path) = output_artifact_path(child).await {
            entries.push(CompletionArchiveEntry {
                agent,
                result_index: Some(result_index),
                source: ArchiveSource::OutputArtifact,
                path: Some(path),
                text: None,
                truncated: None,
            });
            continue;
        }
        if let Some(path) = existing_file(child.get("sessionFile")).await {
            entries.push(CompletionArchiveEntry {
                agent,
                result_index: Some(result_index),
                source: ArchiveSource::Session,
                path: Some(path),
                text: None,
                truncated: None,
            });
            continue;
        }
        // See the doc above: `finalOutput` is the key that exists; `output` is upstream's spelling,
        // kept so a foreign payload that does carry it still archives its text.
        let output = non_empty(child.get("finalOutput")).or_else(|| non_empty(child.get("output")));
        let error = non_empty(child.get("error"));
        if output.is_some() || error.is_some() {
            // pi `:94` — `Error: <error>` first, then the output, `\n`-separated, empties dropped.
            let joined = [error.map(|error| format!("Error: {error}")), output]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n");
            let bounded = utf8_tail(&joined, ARCHIVE_TEXT_LIMIT_BYTES);
            entries.push(CompletionArchiveEntry {
                agent,
                result_index: Some(result_index),
                source: ArchiveSource::ResultTail,
                path: None,
                text: Some(bounded.text),
                truncated: bounded.truncated.then_some(true),
            });
        }
    }

    if results.is_empty()
        && let Some(path) = existing_file(data.get("sessionFile")).await
    {
        entries.push(CompletionArchiveEntry {
            agent: None,
            result_index: None,
            source: ArchiveSource::Session,
            path: Some(path),
            text: None,
            truncated: None,
        });
    }
    if entries.is_empty()
        && let Some(summary) = non_empty(data.get("summary"))
    {
        let bounded = utf8_tail(&summary, ARCHIVE_TEXT_LIMIT_BYTES);
        entries.push(CompletionArchiveEntry {
            agent: None,
            result_index: None,
            source: ArchiveSource::ResultTail,
            path: None,
            text: Some(bounded.text),
            truncated: bounded.truncated.then_some(true),
        });
    }

    let archive = CompletionArchive {
        version: ArchiveVersion,
        run_id: run_id.clone(),
        created_at,
        entries,
    };
    let archive_path = completion_archive_path(results_dir, run_id);
    crate::background::atomic::write_private_atomic_json(&archive_path, &archive).await?;
    Ok(archive_path)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;

    async fn archive_of(dir: &Path, data: &serde_json::Value) -> CompletionArchive {
        let run = RunId::from_token("r1");
        let path = write_completion_archive(dir, &run, data, 7)
            .await
            .expect("archive writes");
        let bytes = tokio::fs::read(&path).await.expect("archive readable");
        parse_archive(&bytes).expect("archive parses")
    }

    /// [`ARCHIVE_TEXT_LIMIT_BYTES`] + `truncated: Some(true)` + a valid UTF-8 boundary. The text is
    /// built from a multi-byte character so a naive byte slice would leave a partial sequence: the
    /// retained tail must still be well-formed, which is what reusing `BoundedByteTail` buys.
    #[tokio::test]
    async fn an_archive_over_64_kib_is_truncated_and_flagged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // "é" is two bytes, so the 64 KiB cut lands mid-character for half of all offsets.
        let huge = "é".repeat(ARCHIVE_TEXT_LIMIT_BYTES);
        let archive = archive_of(
            tmp.path(),
            &json!({ "results": [{ "agent": "coder", "finalOutput": huge }] }),
        )
        .await;

        assert_eq!(archive.entries.len(), 1);
        let entry = &archive.entries[0];
        assert_eq!(entry.source, ArchiveSource::ResultTail);
        assert_eq!(entry.truncated, Some(true));
        let text = entry.text.as_deref().expect("text retained");
        assert!(
            text.len() <= ARCHIVE_TEXT_LIMIT_BYTES,
            "{} bytes retained",
            text.len()
        );
        assert!(
            !text.contains('\u{fffd}'),
            "the tail must start on a character boundary, never mid-sequence"
        );
        assert!(text.ends_with('é'));

        // And the negative: a value UNDER the limit is not flagged (`utf8.ts:9`'s `<=`).
        let archive = archive_of(
            tmp.path(),
            &json!({ "results": [{ "finalOutput": "short" }] }),
        )
        .await;
        assert_eq!(archive.entries[0].truncated, None);
        assert_eq!(archive.entries[0].text.as_deref(), Some("short"));
    }

    /// The `:76-103` ladder, including the `isFile` guard (`:57-65`): a saved artifact wins over a
    /// session file, a session file wins over inline text, and a path that does not exist (or is a
    /// directory) is skipped rather than referenced.
    #[tokio::test]
    async fn an_archive_prefers_an_existing_artifact_over_inline_text() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let artifact = tmp.path().join("output.md");
        let session = tmp.path().join("session.jsonl");
        tokio::fs::write(&artifact, b"the real output".as_slice())
            .await
            .expect("write artifact");
        tokio::fs::write(&session, b"{}".as_slice())
            .await
            .expect("write session");
        let a_directory = tmp.path().join("not-a-file");
        tokio::fs::create_dir_all(&a_directory)
            .await
            .expect("mkdir");

        let archive = archive_of(
            tmp.path(),
            &json!({ "results": [
                // rung 1 wins even though rungs 2 and 3 are both available
                { "agent": "a", "artifactPaths": { "outputPath": artifact },
                  "sessionFile": session, "finalOutput": "ignored" },
                // rung 2: no artifact, a real session file
                { "agent": "b", "sessionFile": session, "finalOutput": "ignored" },
                // rung 3: both file rungs point at things that are not files
                { "agent": "c", "artifactPaths": { "outputPath": tmp.path().join("gone") },
                  "sessionFile": a_directory, "finalOutput": "inline" },
                // not an object — skipped entirely (`:78`)
                "nonsense",
                // nothing at all to archive — contributes no entry
                { "agent": "d" },
            ] }),
        )
        .await;

        assert_eq!(archive.entries.len(), 3, "{:?}", archive.entries);
        assert_eq!(archive.entries[0].source, ArchiveSource::OutputArtifact);
        assert_eq!(archive.entries[0].path.as_deref(), Some(artifact.as_path()));
        assert_eq!(archive.entries[0].text, None);
        assert_eq!(archive.entries[1].source, ArchiveSource::Session);
        assert_eq!(archive.entries[1].path.as_deref(), Some(session.as_path()));
        assert_eq!(archive.entries[2].source, ArchiveSource::ResultTail);
        assert_eq!(archive.entries[2].text.as_deref(), Some("inline"));
        // `resultIndex` is the child's position in `results`, so a caller can zip entries back
        // against the payload even though skipped children leave gaps.
        assert_eq!(archive.entries[2].result_index, Some(2));
    }

    /// The `finalOutput`/`output` correction. Upstream reads `child.output`, a key no `SingleResult`
    /// declares — ported verbatim, rung 3 would be dead for every SUCCESSFUL child (it would fire
    /// only on `error`). Without this test the divergence is invisible: the archive still writes,
    /// it just never carries the answer.
    #[tokio::test]
    async fn an_archive_retains_a_successful_childs_final_output() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let archive = archive_of(
            tmp.path(),
            &json!({ "results": [{ "agent": "coder", "exitCode": 0, "finalOutput": "42" }] }),
        )
        .await;
        assert_eq!(archive.entries.len(), 1, "a successful child IS archived");
        assert_eq!(archive.entries[0].text.as_deref(), Some("42"));

        // Upstream's spelling still works, so a foreign payload carrying `output` is not lost.
        let archive = archive_of(
            tmp.path(),
            &json!({ "results": [{ "agent": "coder", "output": "from-pi" }] }),
        )
        .await;
        assert_eq!(archive.entries[0].text.as_deref(), Some("from-pi"));

        // And `:94`'s join order: the error first, then the output, `\n`-separated.
        let archive = archive_of(
            tmp.path(),
            &json!({ "results": [{ "error": "boom", "finalOutput": "partial" }] }),
        )
        .await;
        assert_eq!(
            archive.entries[0].text.as_deref(),
            Some("Error: boom\npartial")
        );
    }

    /// The two run-level fallbacks (`:104-114`) and the empty archive (`:115-118`). An archive with
    /// zero entries is STILL written, because the record needs a companion file at the canonical
    /// path or `validate_replay_record` would reject its own writer's output.
    #[tokio::test]
    async fn the_run_level_fallbacks_and_the_empty_archive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session = tmp.path().join("run-session.jsonl");
        tokio::fs::write(&session, b"{}".as_slice())
            .await
            .expect("write session");

        // No children at all: the RUN's own session file.
        let archive = archive_of(tmp.path(), &json!({ "sessionFile": session })).await;
        assert_eq!(archive.entries.len(), 1);
        assert_eq!(archive.entries[0].source, ArchiveSource::Session);
        assert_eq!(archive.entries[0].result_index, None, "no child owns it");

        // No children and no session file: the run summary, if a foreign payload carries one.
        let archive = archive_of(tmp.path(), &json!({ "summary": "all done" })).await;
        assert_eq!(archive.entries[0].source, ArchiveSource::ResultTail);
        assert_eq!(archive.entries[0].text.as_deref(), Some("all done"));

        // Nothing to say at all — still a file, still parseable, still version 1.
        let archive = archive_of(tmp.path(), &json!({})).await;
        assert!(archive.entries.is_empty());
        assert_eq!(archive.run_id, RunId::from_token("r1"));
        assert_eq!(archive.created_at, 7);
    }

    /// `parseArchive` is PER-ENTRY tolerant (`:169-181`): one unusable entry is dropped and the
    /// rest survive. A derived `Vec<CompletionArchiveEntry>` would fail the whole archive, turning
    /// a partially-corrupt file into an error and losing the good entries beside the bad one.
    #[test]
    fn a_partially_corrupt_archive_keeps_its_good_entries() {
        let raw = json!({
            "version": 1, "runId": "r1", "createdAt": 1,
            "entries": [
                { "source": "session", "path": "/a" },
                { "source": "no-such-source", "path": "/b" },   // dropped: unknown source
                "not an object",                                 // dropped
                { "source": "result-tail", "text": "keep", "resultIndex": -1, "agent": 7,
                  "truncated": false },
            ],
        })
        .to_string();
        let archive = parse_archive(raw.as_bytes()).expect("parses");
        assert_eq!(archive.entries.len(), 2);
        assert_eq!(archive.entries[1].text.as_deref(), Some("keep"));
        // Each malformed FIELD coerces to absent without dropping its entry (`:174-179`).
        assert_eq!(archive.entries[1].result_index, None, "-1 is not safe");
        assert_eq!(archive.entries[1].agent, None, "7 is not a string");
        assert_eq!(archive.entries[1].truncated, None, "`false` omits the key");

        // A wrong version and a non-array `entries` both fail the WHOLE parse (`:168`).
        let wrong_version = json!({ "version": 2, "runId": "r1", "createdAt": 1, "entries": [] });
        assert!(parse_archive(wrong_version.to_string().as_bytes()).is_none());
        let not_an_array = json!({ "version": 1, "runId": "r1", "createdAt": 1, "entries": {} });
        assert!(parse_archive(not_an_array.to_string().as_bytes()).is_none());
    }
}
