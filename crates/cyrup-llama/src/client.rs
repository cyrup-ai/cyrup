//! the llama-server router HTTP client and SSE reader (`client.ts`)
//!
//! Port of `packages/coding-agent/src/extensions/llama/client.ts` @v0.99.2-17 (EXT-027). Pi never
//! spawns `llama-server`: this is an HTTP client of an already-running server in router mode. The
//! management endpoints it speaks are `GET /models` (catalog, `?reload=1` to rescan), `GET /props`,
//! `POST /models/load`, `POST /models/unload`, `POST /models` (download from Hugging Face) and the
//! `GET /models/sse` event stream.
//!
//! Progress is reported through a caller-supplied `&(dyn Fn(LlamaProgress) + Send + Sync)`, the
//! analogue of upstream's `onProgress` callback: the `/llama` overlay (`ui.rs`) hands in a closure
//! that forwards into its own state or channel. Cancellation is a [`CancellationToken`], the
//! analogue of upstream's `AbortSignal`; pass a fresh `CancellationToken::new()` for "never".

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use reqwest::Method;
use reqwest::header::CONTENT_TYPE;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use cyrup_provider::stream::sse::build_client_for_target;
use cyrup_provider::{EnvAuthContext, ProviderEnv};

use crate::error::LlamaError;

/// Per-request timeout (`client.ts:174`, `AbortSignal.timeout(15_000)`): covers connecting, the
/// response head and reading the body, as the fetch signal does.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// `unloadAndWait` poll interval (`client.ts:221`).
const UNLOAD_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// `loadAndWait` poll interval (`client.ts:296`).
const LOAD_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// `downloadAndWait` poll interval (`client.ts:341`).
const DOWNLOAD_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// The most a management response body may carry before it is refused. A catalog is kilobytes; the
/// cap keeps a misbehaving or hostile server from growing memory for the length of the 15 s
/// timeout. Upstream reads the body unbounded (`client.ts:177`).
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// The most one SSE frame may buffer while waiting for its blank line (`client.ts:241-259` buffers
/// without limit).
const MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;

// ----------------------------------------------------------------------------------------- types --

/// `LlamaModelStatus` (`client.ts:1`): `unloaded | loading | loaded | downloading | sleeping`.
///
/// Upstream's TypeScript union is not enforced at runtime (`isModelInfo` only requires a string,
/// `client.ts:56-60`), so a newer server's unknown status string is carried in [`Self::Other`] and
/// simply compares unequal to every named state, exactly as it does in JS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LlamaModelStatus {
    /// Present in the catalog, no process running.
    #[default]
    Unloaded,
    /// A child server is starting.
    Loading,
    /// A child server is up and serving.
    Loaded,
    /// The model files are being fetched.
    Downloading,
    /// The child is idle-suspended.
    Sleeping,
    /// A status string this port does not name.
    Other(String),
}

impl LlamaModelStatus {
    /// The wire string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Unloaded => "unloaded",
            Self::Loading => "loading",
            Self::Loaded => "loaded",
            Self::Downloading => "downloading",
            Self::Sleeping => "sleeping",
            Self::Other(other) => other,
        }
    }

    fn from_wire(value: String) -> Self {
        match value.as_str() {
            "unloaded" => Self::Unloaded,
            "loading" => Self::Loading,
            "loaded" => Self::Loaded,
            "downloading" => Self::Downloading,
            "sleeping" => Self::Sleeping,
            _ => Self::Other(value),
        }
    }
}

impl Serialize for LlamaModelStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LlamaModelStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from_wire)
    }
}

/// Deserialize an optional field leniently: a value of the wrong type becomes `None` instead of
/// failing the whole catalog. Upstream passes the parsed JSON through untouched and only validates
/// `id` and `status.value` (`client.ts:56-60`), so an odd optional field never breaks `list()`.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok()))
}

/// `Number(element)` as the text [`crate::model`]'s argument scan feeds back through `Number(...)`
/// again: a string stays as it is, a number is its decimal text, `true`/`false`/`null` are the
/// `1`/`0`/`0` JS coerces them to, a one-element array is its element, and anything else is `NaN`.
fn js_argument_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(true) => "1".to_string(),
        Value::Bool(false) | Value::Null => "0".to_string(),
        Value::Array(items) => match items.as_slice() {
            [] => "0".to_string(),
            [only] => js_argument_text(only),
            _ => "NaN".to_string(),
        },
        Value::Object(_) => "NaN".to_string(),
    }
}

/// `status.args` keeps every element. Upstream reads the raw array and tests each element on its
/// own (`configuredContextWindow`, `provider.ts:59-66`: `args[index] !== flag`,
/// `Number(args[index + 1])`), so `["--ctx-size", 4096]` still pins 4096 and one odd element never
/// drops the rest. A non-array value reads as no arguments (`status.args ?? []` iterates nothing).
/// Non-string elements are carried as the text JS's `Number(...)` would read them from (see
/// [`js_argument_text`]); none of them can equal a flag, which is a string comparison.
fn lenient_args<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer) {
        Ok(Value::Array(items)) => Some(items.iter().map(js_argument_text).collect()),
        _ => None,
    })
}

/// A count (`n_ctx`, `n_ctx_train`, `size`): any positive JSON number, as upstream accepts
/// (`runtimeContextWindow && runtimeContextWindow > 0`, `provider.ts:70-76`), so `4096.0` and
/// `4e3` read as the integers they are and a fraction truncates. A non-number, a negative, zero or
/// non-finite value reads as absent.
fn lenient_count<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let Ok(Value::Number(number)) = Value::deserialize(deserializer) else {
        return Ok(None);
    };
    if let Some(count) = number.as_u64() {
        return Ok(Some(count));
    }
    // `{:.0}` of the floor prints the integer's digits without a cast; a value beyond `u64` fails
    // the parse and reads as absent.
    Ok(number
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .and_then(|value| format!("{:.0}", value.floor()).parse::<u64>().ok()))
}

/// `status.exit_code` is present whenever the key is (`entry?.status.exit_code === undefined`,
/// `client.ts:291`): `null`, a fraction or a string is a code like any other and reads back as the
/// JSON value, so `Model exited with code <value>` is built from it.
fn present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

/// `${value}` for an exit code: a string as it is, a number as JS prints it (`3`, `1.5`), `null`,
/// `true` and the like by their JSON text.
fn exit_code_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => match number.as_f64() {
            Some(float) if float.fract() == 0.0 && float.is_finite() => format!("{float:.0}"),
            _ => number.to_string(),
        },
        other => other.to_string(),
    }
}

/// Done/total bytes of one file of a download (`client.ts:11`,
/// `Record<string, { done: number; total: number }>` values).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LlamaFileProgress {
    /// Bytes fetched so far.
    pub done: f64,
    /// Bytes expected.
    pub total: f64,
}

/// Keep the entries of a `progress` object that carry numeric `done` and `total`;
/// `parseDownloadProgress` skips every other entry anyway (`client.ts:121-122`).
fn lenient_progress<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, LlamaFileProgress>>, D::Error>
where
    D: Deserializer<'de>,
{
    let Ok(Value::Object(entries)) = Value::deserialize(deserializer) else {
        return Ok(None);
    };
    Ok(Some(
        entries
            .into_iter()
            .filter_map(|(file, value)| {
                serde_json::from_value::<LlamaFileProgress>(value)
                    .ok()
                    .map(|progress| (file, progress))
            })
            .collect(),
    ))
}

/// The `status` object of a catalog entry (`client.ts:6-12`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaModelStatusInfo {
    /// `status.value`.
    pub value: LlamaModelStatus,
    /// `status.args`: the child server's command line.
    #[serde(
        default,
        deserialize_with = "lenient_args",
        skip_serializing_if = "Option::is_none"
    )]
    pub args: Option<Vec<String>>,
    /// `status.failed`: the last load attempt failed.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub failed: Option<bool>,
    /// `status.exit_code`: the failed child's exit code, as the server sent it (see
    /// [`present_value`]). Only an absent key means "no code".
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub exit_code: Option<Value>,
    /// `status.progress`: per-file download progress while `downloading`.
    ///
    /// **No llama.cpp release puts it here** (EXT-100). `get_router_models` builds `status` from
    /// `value` and `args`, plus `preset` for a preset entry and `exit_code`/`failed` for a failed
    /// one (`llama.cpp@b11436 tools/server/server-models.cpp:2095-2111`); the per-file progress the
    /// router does keep (`server-models.h:83-84`) reaches `GET /models` only through the
    /// `loaded_info` merge at `:2128-2135`, which is gated on `is_running()` — `loaded`, `loading`
    /// or `sleeping`, never `downloading` (`server-models.h:94-96`) — and lands at the entry's TOP
    /// level, not inside `status`. Unchanged at `b9000`, `b10000`, `b10700`, `b11000`, `b11436`.
    ///
    /// The field stays because pi declares it (`client.ts:11` @v0.99.2-17) and this crate ports pi; a
    /// real server reports download progress over SSE instead
    /// (`cyrup_llama_cpp_wire::router::download_progress_event`). What a test feeding
    /// `status.progress` proves is that the port is faithful to pi, NOT that any server does this.
    #[serde(
        default,
        deserialize_with = "lenient_progress",
        skip_serializing_if = "Option::is_none"
    )]
    pub progress: Option<BTreeMap<String, LlamaFileProgress>>,
}

/// `architecture` of a catalog entry (`client.ts:13-16`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaArchitecture {
    /// `architecture.input_modalities` (`text`, `image`, ...).
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub input_modalities: Option<Vec<String>>,
    /// `architecture.output_modalities`.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_modalities: Option<Vec<String>>,
}

/// `meta` of a catalog entry (`client.ts:18-23`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LlamaModelMeta {
    /// `meta.n_ctx`: the context window of the running instance.
    #[serde(
        default,
        deserialize_with = "lenient_count",
        skip_serializing_if = "Option::is_none"
    )]
    pub n_ctx: Option<u64>,
    /// `meta.n_ctx_train`: the trained context length.
    #[serde(
        default,
        deserialize_with = "lenient_count",
        skip_serializing_if = "Option::is_none"
    )]
    pub n_ctx_train: Option<u64>,
    /// `meta.size`: bytes on disk.
    #[serde(
        default,
        deserialize_with = "lenient_count",
        skip_serializing_if = "Option::is_none"
    )]
    pub size: Option<u64>,
    /// `meta.ftype`: the quantization file type.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub ftype: Option<String>,
}

/// One entry of `GET /models` (`LlamaModelInfo`, `client.ts:3-24`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaModelInfo {
    /// Catalog id, the value the load/unload/download endpoints take as `model`.
    pub id: String,
    /// Alternative names.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub aliases: Option<Vec<String>>,
    /// Lifecycle state.
    pub status: LlamaModelStatusInfo,
    /// Input/output modalities.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub architecture: Option<LlamaArchitecture>,
    /// Where the model came from (cache, preset, ...).
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub source: Option<String>,
    /// GGUF metadata the server reports.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub meta: Option<LlamaModelMeta>,
}

/// `LlamaModelsResponse` (`client.ts:26-29`): the `GET /models` body.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LlamaModelsResponse {
    /// The catalog.
    pub data: Vec<LlamaModelInfo>,
    /// `object`, `"list"` on a real server.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub object: Option<String>,
}

/// `LlamaServerProps` (`client.ts:31-34`): the two `GET /props` fields pi reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlamaServerProps {
    /// `models_autoload`.
    pub models_autoload: Option<bool>,
    /// `chat_template`.
    pub chat_template: Option<String>,
}

/// One decoded `/models/sse` event (`LlamaModelEvent`, `client.ts:36-40`).
#[derive(Debug, Clone, PartialEq)]
pub struct LlamaModelEvent {
    /// The model id the event is about.
    pub model: String,
    /// `model_status`, `status_change`, `download_progress`, `download_finished`,
    /// `download_failed`, ...
    pub event: String,
    /// The event payload, when present.
    pub data: Option<Value>,
}

/// One optional member of a [`LlamaProgress`] as `Object.assign(state, progress)` sees it
/// (`ui.ts:510`): the key is either absent, and the target keeps what it had, or present, and the
/// target takes it, `undefined` included.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ProgressField<T> {
    /// The key is absent: an earlier value stays.
    #[default]
    Keep,
    /// The key is present with `undefined`: an earlier value is cleared.
    Clear,
    /// The key is present with a value.
    Set(T),
}

impl<T> ProgressField<T> {
    /// A key that is always present: its value, or `undefined` for `None`.
    #[must_use]
    pub fn from_option(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        }
    }

    /// `Object.assign` of this member onto `target`.
    pub fn apply_to(self, target: &mut Option<T>) {
        match self {
            Self::Keep => {}
            Self::Clear => *target = None,
            Self::Set(value) => *target = Some(value),
        }
    }
}

/// Progress shown by the `/llama` overlay (`LlamaProgress`, `client.ts:42-46`).
///
/// `ratio` and `detail` are optional keys upstream, and what the view does with an update depends
/// on whether the key is there (`ProgressState::apply`), so they carry that: see
/// [`ProgressField`]. A bare message leaves both keys out, so the progress bar and detail line the
/// SSE watcher already delivered stay on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct LlamaProgress {
    /// Headline, e.g. `Loading text model`.
    pub message: String,
    /// `0.0..=1.0` when the server reports enough to compute it.
    pub ratio: ProgressField<f64>,
    /// Secondary line, e.g. `512 B / 1.00 KiB`.
    pub detail: ProgressField<String>,
}

impl LlamaProgress {
    /// A bare message with no `ratio` or `detail` key (`onProgress({ message: "Loading model" })`).
    #[must_use]
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ratio: ProgressField::Keep,
            detail: ProgressField::Keep,
        }
    }
}

/// Progress callback type taken by [`LlamaClient::load_and_wait`] and
/// [`LlamaClient::download_and_wait`] (upstream `onProgress`).
pub type ProgressFn<'a> = &'a (dyn Fn(LlamaProgress) + Send + Sync);

// ------------------------------------------------------------------------------------- helpers --

/// `errorMessage` (`client.ts:48-54`): `payload.error.message` when it is a non-empty string,
/// otherwise `fallback`.
fn error_message(payload: Option<&Value>, fallback: &str) -> String {
    payload
        .and_then(|payload| payload.get("error"))
        .filter(|error| error.is_object() || error.is_array())
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// `isModelInfo` (`client.ts:56-60`): a string `id` and a string `status.value`.
fn is_model_info(value: &Value) -> bool {
    value.get("id").is_some_and(Value::is_string)
        && value
            .get("status")
            .and_then(|status| status.get("value"))
            .is_some_and(Value::is_string)
}

/// `sleep` (`client.ts:73-89`): wait `duration`, or fail the moment the caller cancels.
async fn sleep(duration: Duration, cancel: &CancellationToken) -> Result<(), LlamaError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(LlamaError::Cancelled),
        () = tokio::time::sleep(duration) => Ok(()),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Run `main` with `watcher` polled alongside it; `main` alone decides the result.
///
/// Upstream starts the SSE reader as a detached promise whose failure is swallowed
/// (`void this.watch(...).catch(() => {})`, `client.ts:272-280`, `:315-324`) and aborts it in
/// `finally`. Here the reader is a future polled by the same task, so it ends exactly when `main`
/// returns (the select drops it) and a failed or finished reader simply stops contributing events
/// while `main`'s catalog polling stays authoritative. The watcher is polled first so the SSE
/// request starts before the load/download POST, the order upstream's `void` call produces.
async fn with_watcher<T>(watcher: impl Future<Output = ()>, main: impl Future<Output = T>) -> T {
    let watcher = async {
        watcher.await;
        std::future::pending::<T>().await
    };
    tokio::select! {
        biased;
        out = watcher => out,
        out = main => out,
    }
}

/// The JSON values of an object, or the elements of an array (`Object.values` accepts both).
fn object_values(value: &Value) -> Vec<&Value> {
    match value {
        Value::Object(map) => map.values().collect(),
        Value::Array(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

/// `parseLoadProgress` (`client.ts:91-111`).
///
/// `data.progress` is `{ stages: [..], current|stage: "..", value: 0..1 }`; the ratio is
/// `(stageIndex + value) / stages` when the current stage is one of `stages`, otherwise the bare
/// clamped `value`. `current` wins over `stage` (`:97`); an empty stage name is falsy upstream
/// (`:103`, `:108`) and so reads as no stage.
pub(crate) fn parse_load_progress(data: &Value) -> Option<LlamaProgress> {
    if !(data.is_object() || data.is_array()) {
        return None;
    }
    let progress = data.get("progress")?;
    if !(progress.is_object() || progress.is_array()) {
        return None;
    }
    let stage = progress
        .get("current")
        .and_then(Value::as_str)
        .or_else(|| progress.get("stage").and_then(Value::as_str))
        .filter(|stage| !stage.is_empty());
    let stages: Vec<&str> = progress
        .get("stages")
        .and_then(Value::as_array)
        .map(|stages| stages.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let stage_ratio = progress
        .get("value")
        .and_then(Value::as_f64)
        .map(|value| value.clamp(0.0, 1.0));
    let mut ratio = stage_ratio;
    if let Some(stage) = stage
        && !stages.is_empty()
        && let Some(index) = stages.iter().position(|candidate| *candidate == stage)
    {
        #[allow(clippy::cast_precision_loss)]
        let (index, count) = (index as f64, stages.len() as f64);
        ratio = Some((index + stage_ratio.unwrap_or(0.0)) / count);
    }
    // `{ message, ratio }` (`client.ts:107-110`): `ratio` is always an own key, `undefined` when
    // there is no value, and there is no `detail` key.
    Some(LlamaProgress {
        message: match stage {
            Some(stage) => format!("Loading {}", stage.replace('_', " ")),
            None => "Loading model".to_string(),
        },
        ratio: ProgressField::from_option(ratio),
        detail: ProgressField::Keep,
    })
}

/// Sum per-file `done`/`total` into one [`LlamaProgress`] (`client.ts:117-131`); `None` when the
/// total is not positive.
fn download_progress_from(files: impl IntoIterator<Item = (f64, f64)>) -> Option<LlamaProgress> {
    let (mut done, mut total) = (0.0, 0.0);
    for (file_done, file_total) in files {
        done += file_done;
        total += file_total;
    }
    if total <= 0.0 {
        return None;
    }
    Some(LlamaProgress {
        message: "Downloading model".to_string(),
        ratio: ProgressField::Set(done / total),
        detail: ProgressField::Set(format!("{} / {}", format_bytes(done), format_bytes(total))),
    })
}

/// `parseDownloadProgress` (`client.ts:113-132`) for an SSE event payload: either
/// `{ progress: { <file>: { done, total } } }` (nested) or the bare `{ <file>: { done, total } }`
/// (flat); entries without numeric `done` and `total` are skipped.
pub(crate) fn parse_download_progress(data: &Value) -> Option<LlamaProgress> {
    if !(data.is_object() || data.is_array()) {
        return None;
    }
    let files = match data.get("progress") {
        Some(nested) if nested.is_object() || nested.is_array() => nested,
        _ => data,
    };
    download_progress_from(
        object_values(files).into_iter().filter_map(|entry| {
            Some((entry.get("done")?.as_f64()?, entry.get("total")?.as_f64()?))
        }),
    )
}

/// `toFixed` of a non-negative number with JS semantics: round half up on the exact binary value
/// (Rust's `{:.N}` rounds exact ties to even, so `1.125` would print `1.12` where JS prints `1.13`).
fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() || value < 0.0 {
        return value.to_string();
    }
    // Exact decimal expansion: every f64 below 2^53 has at most 52 fractional binary digits.
    let text = format!("{value:.60}");
    let (int_part, frac_part) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let mut digits_kept: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes().take(digits))
        .collect();
    if frac_part
        .as_bytes()
        .get(digits)
        .is_some_and(|digit| *digit >= b'5')
    {
        let mut carry = true;
        for digit in digits_kept.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            digits_kept.insert(0, b'1');
        }
    }
    let int_len = digits_kept.len().saturating_sub(digits);
    let (int_digits, frac_digits) = digits_kept.split_at(int_len);
    let mut out = String::from_utf8_lossy(int_digits).into_owned();
    if digits > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(frac_digits));
    }
    out
}

/// `formatBytes` (`client.ts:134-144`): `512 B`, `1.00 KiB`, `12.3 MiB`: two decimals under 10,
/// one from 10, binary units up to TiB.
///
/// Takes `f64` because upstream's `number` does (download progress sums are fractional-capable);
/// pass a `u64` size with `as f64`.
#[must_use]
pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        return format!("{bytes} B");
    }
    let units = ["KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes / 1024.0;
    let mut unit = "KiB";
    for next in units.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    let digits = if value >= 10.0 { 1 } else { 2 };
    format!("{} {unit}", to_fixed(value, digits))
}

/// `normalizeLlamaServerUrl` (`client.ts:146-155`): `http`/`https` only; drops the fragment, the
/// query, trailing slashes and a trailing `/v1`; the result carries no trailing slash.
///
/// # Errors
///
/// `Invalid URL` when `value` does not parse (`new URL` throws), `Server URL must use http or
/// https` for any other scheme, `Server URL must not include credentials` when it carries a user
/// name or password.
pub fn normalize_llama_server_url(value: &str) -> Result<String, LlamaError> {
    let mut url = Url::parse(value.trim()).map_err(|_| LlamaError::message("Invalid URL"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(LlamaError::message("Server URL must use http or https"));
    }
    // Node's `fetch` refuses a URL that carries credentials ("Request cannot be constructed from a
    // URL that includes credentials"), and its message repeats the URL. Refusing here, without the
    // URL in the text, keeps a password out of the persisted `LLAMA_BASE_URL`, the `/login` failure
    // and every `/llama` notification.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(LlamaError::message(
            "Server URL must not include credentials",
        ));
    }
    url.set_fragment(None);
    url.set_query(None);
    let trimmed = url.path().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    let path = if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    };
    url.set_path(&path);
    let text = url.to_string();
    Ok(text.strip_suffix('/').unwrap_or(&text).to_string())
}

/// `llamaInferenceUrl` (`client.ts:157-159`): the OpenAI-compatible base URL, `<server>/v1`.
///
/// # Errors
///
/// As [`normalize_llama_server_url`].
pub fn llama_inference_url(server_url: &str) -> Result<String, LlamaError> {
    Ok(format!("{}/v1", normalize_llama_server_url(server_url)?))
}

// ------------------------------------------------------------------------------------ sse reader --

/// A streaming UTF-8 decoder (`new TextDecoder().decode(chunk, { stream: true })`,
/// `client.ts:240`): a multi-byte sequence split across chunks is held back until complete, an
/// invalid sequence becomes U+FFFD, and one leading U+FEFF at the start of the stream is consumed
/// (`TextDecoder`'s default `ignoreBOM: false`), so a BOM-prefixed first frame still starts with
/// `data:`.
#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
    /// Whether the start of the stream has been decoded, so the BOM is no longer possible.
    started: bool,
}

impl Utf8Decoder {
    fn decode(&mut self, chunk: &[u8]) -> String {
        let mut text = self.decode_chunk(chunk);
        if !self.started && !text.is_empty() {
            self.started = true;
            if let Some(rest) = text.strip_prefix('\u{FEFF}') {
                text = rest.to_string();
            }
        }
        text
    }

    fn decode_chunk(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let bytes = std::mem::take(&mut self.pending);
        let mut rest = bytes.as_slice();
        let mut out = String::new();
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    out.push_str(&String::from_utf8_lossy(valid));
                    match error.error_len() {
                        Some(len) => {
                            out.push('\u{FFFD}');
                            rest = after.get(len..).unwrap_or_default();
                        }
                        None => {
                            self.pending = after.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }
}

/// Decode every complete `\n\n`-terminated frame in `buffer` (`client.ts:241-259`): the `data:`
/// lines of a frame are joined with `\n`, parsed as JSON, and delivered when they carry a string
/// `model` and a string `event`; a malformed frame is ignored ("catalog polling remains
/// authoritative", `:255`).
fn drain_frames(buffer: &mut String, on_event: &mut impl FnMut(LlamaModelEvent)) {
    while let Some(boundary) = buffer.find("\n\n") {
        let rest = buffer.split_off(boundary + 2);
        let mut frame = std::mem::replace(buffer, rest);
        frame.truncate(boundary);
        let data = frame
            .split('\n')
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let Ok(Value::Object(mut event)) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let (Some(Value::String(model)), Some(Value::String(name))) =
            (event.remove("model"), event.remove("event"))
        else {
            continue;
        };
        on_event(LlamaModelEvent {
            model,
            event: name,
            data: event.remove("data"),
        });
    }
}

// ------------------------------------------------------------------------------------ body read --

/// What reading a bounded response body produced.
pub(crate) enum BodyRead {
    /// The whole body, within the limit.
    Complete(Vec<u8>),
    /// The connection failed mid-body; callers treat it as no payload, as `.catch(() => undefined)`
    /// does upstream (`client.ts:177-182`).
    Unreadable,
    /// The body is longer than the limit (by `content-length` or by what arrived).
    TooLarge,
}

/// Read `response`'s body up to `limit` bytes, stopping at the first byte past it.
pub(crate) async fn read_capped_body(response: reqwest::Response, limit: usize) -> BodyRead {
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return BodyRead::TooLarge;
    }
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return BodyRead::Unreadable;
        };
        if body.len().saturating_add(chunk.len()) > limit {
            return BodyRead::TooLarge;
        }
        body.extend_from_slice(&chunk);
    }
    BodyRead::Complete(body)
}

// ---------------------------------------------------------------------------------------- client --

/// HTTP client of one llama-server (`LlamaClient`, `client.ts:161-348`).
#[derive(Clone)]
pub struct LlamaClient {
    server_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
    timeout: Duration,
}

impl std::fmt::Debug for LlamaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlamaClient")
            .field("server_url", &self.server_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl LlamaClient {
    /// `new LlamaClient(serverUrl, apiKey?)` (`client.ts:165-168`). The URL is normalised
    /// ([`normalize_llama_server_url`]); an empty `api_key` is no key (`if (this.apiKey)`,
    /// `:173`).
    ///
    /// The HTTP client resolves its proxy the way every other HTTP path of the workspace does,
    /// through [`build_client_for_target`]: `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY` for this server's
    /// URL, with `env` (the provider-scoped overlay) winning over the process environment, as pi's
    /// global `EnvHttpProxyAgent` dispatcher does for its `fetch` (`core/http-dispatcher.ts:87-100`).
    ///
    /// # Errors
    ///
    /// The URL errors of [`normalize_llama_server_url`], or a transport error when the proxy
    /// setting is unusable or the HTTP stack cannot be initialised.
    pub async fn new(
        server_url: &str,
        api_key: Option<String>,
        env: Option<&ProviderEnv>,
    ) -> Result<Self, LlamaError> {
        let server_url = normalize_llama_server_url(server_url)?;
        let http = build_client_for_target(&server_url, &EnvAuthContext, env, None)
            .await
            .map_err(|error| LlamaError::transport(&error))?;
        Self::with_http_client(&server_url, api_key, http)
    }

    /// [`Self::new`] over a caller-supplied `reqwest::Client` (proxy, TLS or test configuration).
    ///
    /// # Errors
    ///
    /// The URL errors of [`normalize_llama_server_url`].
    pub fn with_http_client(
        server_url: &str,
        api_key: Option<String>,
        http: reqwest::Client,
    ) -> Result<Self, LlamaError> {
        Ok(Self {
            server_url: normalize_llama_server_url(server_url)?,
            api_key: api_key.filter(|key| !key.is_empty()),
            http,
            timeout: REQUEST_TIMEOUT,
        })
    }

    /// Replace the 15 s per-request timeout (`client.ts:174`); chiefly for tests.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The normalised server URL (`serverUrl`, `client.ts:162`), no trailing slash, no `/v1`.
    #[must_use]
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// The per-request timeout in force (15 s unless [`Self::with_request_timeout`] changed it).
    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        self.timeout
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    /// `request` (`client.ts:170-185`): one JSON call with the 15 s timeout combined with the
    /// caller's cancellation. A body that is not JSON reads as no payload (`:177-182`); a non-2xx
    /// status fails with the server's `error.message` or `llama.cpp returned HTTP <n>` (`:183`).
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<Option<Value>, LlamaError> {
        let work = async {
            let mut request = self
                .http
                .request(method, format!("{}{path}", self.server_url));
            if let Some(body) = body {
                request = request.header(CONTENT_TYPE, "application/json").body(body);
            }
            let response = self
                .authorize(request)
                .send()
                .await
                .map_err(LlamaError::from_reqwest)?;
            let status = response.status();
            let payload = match read_capped_body(response, MAX_BODY_BYTES).await {
                BodyRead::Complete(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
                BodyRead::Unreadable => None,
                BodyRead::TooLarge => {
                    return Err(LlamaError::Message(format!(
                        "llama.cpp response exceeds {MAX_BODY_BYTES} bytes"
                    )));
                }
            };
            if !status.is_success() {
                return Err(LlamaError::Message(error_message(
                    payload.as_ref(),
                    &format!("llama.cpp returned HTTP {}", status.as_u16()),
                )));
            }
            Ok(payload)
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(LlamaError::Cancelled),
            out = tokio::time::timeout(self.timeout, work) => {
                out.map_err(|_| LlamaError::Timeout)?
            }
        }
    }

    /// `GET /models[?reload=1]` (`list`, `client.ts:187-195`).
    ///
    /// # Errors
    ///
    /// `llama.cpp returned an invalid model catalog` when the body has no `data` array,
    /// `Server is not running in llama.cpp router mode` when an entry lacks `id` or
    /// `status.value` (a plain, non-router server), or the request errors.
    pub async fn list(
        &self,
        reload: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<LlamaModelInfo>, LlamaError> {
        let path = if reload {
            "/models?reload=1"
        } else {
            "/models"
        };
        let payload = self.request(Method::GET, path, None, cancel).await?;
        let invalid = || LlamaError::message("llama.cpp returned an invalid model catalog");
        let Some(Value::Array(data)) =
            payload.and_then(|mut payload| payload.get_mut("data").map(Value::take))
        else {
            return Err(invalid());
        };
        if !data.iter().all(is_model_info) {
            return Err(LlamaError::message(
                "Server is not running in llama.cpp router mode",
            ));
        }
        data.into_iter()
            .map(|entry| serde_json::from_value(entry).map_err(|_| invalid()))
            .collect()
    }

    /// `GET /props[?model=ID&autoload=false]` (`props`, `client.ts:197-206`). With a model the
    /// router answers for that child without loading it. Only a boolean `models_autoload` and a
    /// string `chat_template` are kept.
    ///
    /// # Errors
    ///
    /// The request errors.
    pub async fn props(
        &self,
        model: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<LlamaServerProps, LlamaError> {
        let query = match model.filter(|model| !model.is_empty()) {
            Some(model) => format!(
                "?{}",
                url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("model", model)
                    .append_pair("autoload", "false")
                    .finish()
            ),
            None => String::new(),
        };
        let payload = self
            .request(Method::GET, &format!("/props{query}"), None, cancel)
            .await?;
        let Some(payload) = payload.filter(|payload| payload.is_object() || payload.is_array())
        else {
            return Ok(LlamaServerProps::default());
        };
        Ok(LlamaServerProps {
            models_autoload: payload.get("models_autoload").and_then(Value::as_bool),
            chat_template: payload
                .get("chat_template")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    async fn post_model(
        &self,
        path: &str,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<(), LlamaError> {
        let body = serde_json::json!({ "model": model }).to_string();
        self.request(Method::POST, path, Some(body), cancel)
            .await
            .map(|_| ())
    }

    /// `POST /models/load` (`load`, `client.ts:208-210`): ask the router to start the model; it
    /// answers before the child is up.
    ///
    /// # Errors
    ///
    /// The request errors.
    pub async fn load(&self, model: &str, cancel: &CancellationToken) -> Result<(), LlamaError> {
        self.post_model("/models/load", model, cancel).await
    }

    /// `POST /models/unload` (`unload`, `client.ts:212-214`).
    ///
    /// # Errors
    ///
    /// The request errors.
    pub async fn unload(&self, model: &str, cancel: &CancellationToken) -> Result<(), LlamaError> {
        self.post_model("/models/unload", model, cancel).await
    }

    /// `unloadAndWait` (`client.ts:216-223`): unload, then poll the catalog every 100 ms until the
    /// model is `unloaded` or has left the catalog.
    ///
    /// # Errors
    ///
    /// The request errors, or cancellation.
    pub async fn unload_and_wait(
        &self,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<(), LlamaError> {
        self.unload(model, cancel).await?;
        loop {
            let models = self.list(false, cancel).await?;
            let entry = models.iter().find(|candidate| candidate.id == model);
            if entry.is_none_or(|entry| entry.status.value == LlamaModelStatus::Unloaded) {
                return Ok(());
            }
            sleep(UNLOAD_POLL_INTERVAL, cancel).await?;
        }
    }

    /// `POST /models` (`download`, `client.ts:225-227`): ask the router to fetch a Hugging Face
    /// model (`owner/repo[:quant]`).
    ///
    /// # Errors
    ///
    /// The request errors.
    pub async fn download(
        &self,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<(), LlamaError> {
        self.post_model("/models", model, cancel).await
    }

    /// `watch` (`client.ts:229-261`): read `GET /models/sse`, calling `on_event` for every
    /// well-formed `{model, event, data?}` frame, until the stream ends (`Ok`), fails, or the
    /// caller cancels. Hand-rolled SSE: frames end at a blank line and only their `data:` lines
    /// matter. No request timeout applies; the stream is long-lived.
    ///
    /// # Errors
    ///
    /// `llama.cpp SSE returned HTTP <n>` for a non-2xx answer, a transport error, or cancellation.
    pub async fn watch(
        &self,
        mut on_event: impl FnMut(LlamaModelEvent) + Send,
        cancel: &CancellationToken,
    ) -> Result<(), LlamaError> {
        let request = self.authorize(self.http.get(format!("{}/models/sse", self.server_url)));
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(LlamaError::Cancelled),
            response = request.send() => response.map_err(LlamaError::from_reqwest)?,
        };
        if !response.status().is_success() {
            return Err(LlamaError::Message(format!(
                "llama.cpp SSE returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let mut stream = response.bytes_stream();
        let mut decoder = Utf8Decoder::default();
        let mut buffer = String::new();
        loop {
            let chunk = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(LlamaError::Cancelled),
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else {
                return Ok(());
            };
            let chunk = chunk.map_err(LlamaError::from_reqwest)?;
            buffer.push_str(&decoder.decode(&chunk).replace("\r\n", "\n"));
            drain_frames(&mut buffer, &mut on_event);
            if buffer.len() > MAX_SSE_FRAME_BYTES {
                return Err(LlamaError::Message(format!(
                    "llama.cpp SSE frame exceeds {MAX_SSE_FRAME_BYTES} bytes"
                )));
            }
        }
    }

    /// `loadAndWait` (`client.ts:263-302`): load a model and wait until it is serving, reporting
    /// progress.
    ///
    /// The SSE stream ([`Self::watch`]) and 250 ms catalog polling run concurrently. SSE supplies
    /// stage progress (`Loading text model`, ratio `(stageIndex + value) / stages`) and an early
    /// `loaded` / `unloaded` signal; polling is authoritative, so an SSE stream that fails or never
    /// connects costs only the progress detail. Fails on `status.failed`, on an SSE `unloaded`
    /// (`Model failed to load`), and with `Model exited with code N` when the failed entry carries
    /// an exit code.
    ///
    /// # Errors
    ///
    /// The failures above, the request errors, or cancellation.
    pub async fn load_and_wait(
        &self,
        model: &str,
        on_progress: ProgressFn<'_>,
        cancel: &CancellationToken,
    ) -> Result<LlamaModelInfo, LlamaError> {
        let event_loaded = AtomicBool::new(false);
        let event_error: Mutex<Option<String>> = Mutex::new(None);
        let watcher = async {
            // A failed SSE stream is swallowed (`.catch(() => {})`, `client.ts:280`).
            let _ = self
                .watch(
                    |event| {
                        if event.model != model {
                            return;
                        }
                        if event.event != "model_status" && event.event != "status_change" {
                            return;
                        }
                        let status = event
                            .data
                            .as_ref()
                            .and_then(|data| data.get("status"))
                            .and_then(Value::as_str);
                        if status == Some("loaded") {
                            event_loaded.store(true, Ordering::SeqCst);
                        }
                        if status == Some("unloaded") {
                            *lock(&event_error) = Some("Model failed to load".to_string());
                        }
                        if let Some(progress) = event.data.as_ref().and_then(parse_load_progress) {
                            on_progress(progress);
                        }
                    },
                    cancel,
                )
                .await;
        };
        let poll = async {
            self.load(model, cancel).await?;
            on_progress(LlamaProgress::message("Loading model"));
            loop {
                if cancel.is_cancelled() {
                    return Err(LlamaError::Cancelled);
                }
                let models = self.list(false, cancel).await?;
                let entry = models.into_iter().find(|candidate| candidate.id == model);
                if let Some(entry) = &entry
                    && entry.status.value == LlamaModelStatus::Loaded
                {
                    return Ok(entry.clone());
                }
                if event_loaded.load(Ordering::SeqCst) && entry.is_none() {
                    return Ok(LlamaModelInfo {
                        id: model.to_string(),
                        status: LlamaModelStatusInfo {
                            value: LlamaModelStatus::Loaded,
                            ..LlamaModelStatusInfo::default()
                        },
                        ..LlamaModelInfo::default()
                    });
                }
                let event_error = lock(&event_error).clone();
                let failed = entry
                    .as_ref()
                    .is_some_and(|entry| entry.status.failed == Some(true));
                if failed || event_error.is_some() {
                    return Err(LlamaError::Message(
                        match entry
                            .as_ref()
                            .and_then(|entry| entry.status.exit_code.as_ref())
                        {
                            None => {
                                event_error.unwrap_or_else(|| "Model failed to load".to_string())
                            }
                            Some(code) => {
                                format!("Model exited with code {}", exit_code_text(code))
                            }
                        },
                    ));
                }
                sleep(LOAD_POLL_INTERVAL, cancel).await?;
            }
        };
        with_watcher(watcher, poll).await
    }

    /// `downloadAndWait` (`client.ts:304-347`): start a download and wait for it, reporting byte
    /// progress; returns the catalog re-read with `?reload=1`.
    ///
    /// SSE `download_progress` / `download_finished` / `download_failed` events run alongside 500 ms
    /// catalog polling (a `downloading` entry also reports progress). Done when a
    /// `download_finished` arrives, or the entry exists and either a download was observed or two
    /// polls have passed (a cached model downloads instantly and never shows `downloading`).
    ///
    /// # Errors
    ///
    /// The `download_failed` message (`Download failed` when the event has none), the request
    /// errors, or cancellation.
    pub async fn download_and_wait(
        &self,
        model: &str,
        on_progress: ProgressFn<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<LlamaModelInfo>, LlamaError> {
        let finished = AtomicBool::new(false);
        let saw_downloading = AtomicBool::new(false);
        let failure: Mutex<Option<String>> = Mutex::new(None);
        let watcher = async {
            let _ = self
                .watch(
                    |event| {
                        if event.model != model {
                            return;
                        }
                        match event.event.as_str() {
                            "download_finished" => finished.store(true, Ordering::SeqCst),
                            "download_failed" => {
                                *lock(&failure) =
                                    Some(error_message(event.data.as_ref(), "Download failed"));
                            }
                            "download_progress" => {
                                saw_downloading.store(true, Ordering::SeqCst);
                                if let Some(progress) =
                                    event.data.as_ref().and_then(parse_download_progress)
                                {
                                    on_progress(progress);
                                }
                            }
                            _ => {}
                        }
                    },
                    cancel,
                )
                .await;
        };
        let poll = async {
            self.download(model, cancel).await?;
            on_progress(LlamaProgress::message("Downloading model"));
            let mut polls = 0_u32;
            loop {
                if cancel.is_cancelled() {
                    return Err(LlamaError::Cancelled);
                }
                if let Some(failure) = lock(&failure).clone() {
                    return Err(LlamaError::Message(failure));
                }
                let models = self.list(false, cancel).await?;
                polls += 1;
                let entry = models.iter().find(|candidate| candidate.id == model);
                if let Some(entry) =
                    entry.filter(|entry| entry.status.value == LlamaModelStatus::Downloading)
                {
                    saw_downloading.store(true, Ordering::SeqCst);
                    // `entry.status.progress` is never set by a real router — see
                    // `LlamaModelStatusInfo::progress`. This branch is pi's (`client.ts:334-338`)
                    // and is kept for fidelity; the live progress path is the SSE one above.
                    if let Some(files) = &entry.status.progress
                        && let Some(progress) = download_progress_from(
                            files.values().map(|file| (file.done, file.total)),
                        )
                    {
                        on_progress(progress);
                    }
                } else if finished.load(Ordering::SeqCst)
                    || (entry.is_some() && (saw_downloading.load(Ordering::SeqCst) || polls >= 2))
                {
                    return self.list(true, cancel).await;
                }
                sleep(DOWNLOAD_POLL_INTERVAL, cancel).await?;
            }
        };
        with_watcher(watcher, poll).await
    }
}
