//! Hugging Face search, quantization parsing and token lookup (`huggingface.ts`)
//!
//! Port of `packages/coding-agent/src/extensions/llama/huggingface.ts` @v0.99.2-17 (EXT-027). The
//! `/llama` manager searches Hugging Face for GGUF repositories (`ui.ts:158`), then asks for one
//! repository's quantizations and access requirements before telling the router to download it
//! (`index.ts:138-165`). This module is the whole of that client: the token lookup
//! ([`find_huggingface_token`]), the two read-only API calls ([`HuggingFaceClient::search`],
//! [`HuggingFaceClient::details`]) and the GGUF filename to quantization mapping.
//!
//! Cancellation is a [`CancellationToken`], the analogue of upstream's `AbortSignal`; pass a fresh
//! `CancellationToken::new()` for "never". The token is never logged: [`HuggingFaceClient`]'s
//! `Debug` redacts it, and no message produced here contains it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::client::{BodyRead, MAX_BODY_BYTES, read_capped_body};
use crate::error::LlamaError;
use cyrup_provider::stream::sse::build_client_for_target;
use cyrup_provider::{EnvAuthContext, ProviderEnv};

/// `DEFAULT_HUGGING_FACE_URL` (`huggingface.ts:5`).
pub const DEFAULT_HUGGING_FACE_URL: &str = "https://huggingface.co";

/// Per-request timeout (`huggingface.ts:75`, `AbortSignal.timeout(15_000)`): covers connecting, the
/// response head and reading the body, as the fetch signal does.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// `QUANTIZATION_PATTERN` (`huggingface.ts:6-7`). `\d` is spelled `[0-9]` because the JS pattern's
/// `u` flag keeps `\d` ASCII-only while the Rust `regex` crate's is Unicode.
const QUANTIZATION_PATTERN: &str = r"(?i)(?:^|[-_.])((?:UD-)?(?:IQ[0-9](?:_[A-Z0-9]+)+|Q[0-9](?:_[A-Z0-9]+)+|BF16|F16|F32|MXFP[0-9](?:_[A-Z0-9]+)*))$";
/// `SHARD_SUFFIX_PATTERN` (`huggingface.ts:8`).
const SHARD_SUFFIX_PATTERN: &str = r"-[0-9]{5}-of-[0-9]{5}$";
/// `RATE_LIMIT_DELAY_PATTERN`, the literal of `parseRateLimitDelay` (`huggingface.ts:33`).
const RATE_LIMIT_DELAY_PATTERN: &str = r"(?:^|;)t=([0-9]+)";

fn literal_regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern)
        .unwrap_or_else(|_| unreachable!("a Hugging Face filename pattern is a literal"))
}

static QUANTIZATION: LazyLock<regex::Regex> = LazyLock::new(|| literal_regex(QUANTIZATION_PATTERN));
static SHARD_SUFFIX: LazyLock<regex::Regex> = LazyLock::new(|| literal_regex(SHARD_SUFFIX_PATTERN));
static RATE_LIMIT_DELAY: LazyLock<regex::Regex> =
    LazyLock::new(|| literal_regex(RATE_LIMIT_DELAY_PATTERN));

// ----------------------------------------------------------------------------------------- types --

/// `HuggingFaceModel` (`huggingface.ts:10-13`): one search hit. `downloads` is a JS `number`, so
/// it is an `f64` here.
#[derive(Debug, Clone, PartialEq)]
pub struct HuggingFaceModel {
    /// `owner/repository`.
    pub id: String,
    /// Download count; `0` when the API omits it or sends a non-number (`huggingface.ts:114`).
    pub downloads: f64,
}

/// `HuggingFaceQuantization` (`huggingface.ts:15-18`): one quantization of a repository, summed
/// over its shards.
#[derive(Debug, Clone, PartialEq)]
pub struct HuggingFaceQuantization {
    /// Upper-cased quantization name, for example `Q4_K_M` or `UD-Q4_K_XL`.
    pub name: String,
    /// Total bytes, or `None` when any file of this quantization reported no size
    /// (`huggingface.ts:136-138`, `:143`).
    pub size: Option<f64>,
}

/// The `gated` field of [`HuggingFaceModelDetails`] (`huggingface.ts:22`,
/// `false | "auto" | "manual"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuggingFaceGated {
    /// `false`: anyone can download.
    Open,
    /// `"auto"`: the terms must be accepted on the Hugging Face page.
    Auto,
    /// `"manual"`: the repository's authors must approve the request.
    Manual,
}

impl HuggingFaceGated {
    /// Whether the repository is gated at all (upstream's `if (details.gated)`, `index.ts:144`).
    #[must_use]
    pub fn is_gated(self) -> bool {
        self != Self::Open
    }
}

/// `HuggingFaceModelDetails` (`huggingface.ts:20-24`).
#[derive(Debug, Clone, PartialEq)]
pub struct HuggingFaceModelDetails {
    /// The repository id the API reports, falling back to the requested one (`huggingface.ts:153`).
    pub id: String,
    /// Access requirement (`huggingface.ts:154`).
    pub gated: HuggingFaceGated,
    /// Quantizations: `Q4_K_M` first, then smallest size first, unknown sizes last, then by name
    /// (`huggingface.ts:142-151`).
    pub quantizations: Vec<HuggingFaceQuantization>,
}

// ------------------------------------------------------------------------------- JS-faithful bits --

/// JavaScript `String.prototype.trim`: Unicode white space plus the byte-order mark, which Rust's
/// `trim` does not strip but which a token file written on Windows often starts with.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `encodeURIComponent` (`huggingface.ts:119`): every byte but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is
/// percent-encoded.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The digits of a `0x`/`0o`/`0b` literal as a number (`StringToNumber` accepts the three
/// prefixes, unsigned), `None` when none applies or a digit is invalid.
fn prefixed_integer(text: &str) -> Option<Option<f64>> {
    let (digits, radix) = [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ]
    .into_iter()
    .find_map(|(prefix, radix)| text.strip_prefix(prefix).map(|digits| (digits, radix)))?;
    if digits.is_empty() {
        return Some(None);
    }
    Some(digits.chars().try_fold(0.0_f64, |acc, c| {
        c.to_digit(radix)
            .map(|digit| acc * f64::from(radix) + f64::from(digit))
    }))
}

/// JavaScript `Number(text)` reduced to what the caller tests, the truthiness of the result:
/// `None` for `NaN` and `0` (both falsy), the number otherwise (`huggingface.ts:90`). An empty or
/// blank string is `0`, so it is `None` too.
fn js_truthy_number(text: &str) -> Option<f64> {
    let text = js_trim(text);
    let number = match prefixed_integer(text) {
        Some(parsed) => parsed?,
        None => {
            // Rust also parses `inf`/`nan`; JavaScript only knows `Infinity`.
            let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
            let spelled_out = unsigned == "Infinity"
                || !unsigned
                    .chars()
                    .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E');
            if !spelled_out {
                return None;
            }
            text.parse::<f64>().ok()?
        }
    };
    (number != 0.0 && !number.is_nan()).then_some(number)
}

/// `${number}` for the delay in the rate-limit message (`huggingface.ts:92`): integers print
/// without a fraction, the infinities print as JavaScript spells them.
fn js_number_text(number: f64) -> String {
    if number.is_infinite() {
        return if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_string();
    }
    number.to_string()
}

/// `parseRateLimitDelay` (`huggingface.ts:32-35`): the first `t=<digits>` of an IETF `RateLimit`
/// header value such as `"default";r=0;t=30`. Zero is returned as zero; `request`'s `||`
/// (`:90`) is what treats it as absent.
fn parse_rate_limit_delay(value: Option<&str>) -> Option<f64> {
    let captures = RATE_LIMIT_DELAY.captures(value?)?;
    captures.get(1)?.as_str().parse::<f64>().ok()
}

/// All values of a header joined the way `Headers.get` joins them, `, `.
fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let values: Vec<String> = headers
        .get_all(name)
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

/// `payloadError` (`huggingface.ts:26-30`): the payload's non-empty string `error`, else `fallback`.
fn payload_error(payload: Option<&Value>, fallback: &str) -> String {
    match payload.and_then(|payload| payload.get("error")) {
        Some(Value::String(error)) if !error.is_empty() => error.clone(),
        _ => fallback.to_string(),
    }
}

// ------------------------------------------------------------------------------------ token lookup --

/// The process environment as the string map [`find_huggingface_token`] takes (upstream's default
/// argument `process.env`, `huggingface.ts:46`).
#[must_use]
pub fn process_environment() -> BTreeMap<String, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// `readToken` (`huggingface.ts:37-44`): the trimmed file contents, or `None` when the file cannot
/// be read or holds only white space. Read-only.
async fn read_token(path: &Path) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let token = js_trim(&text);
    (!token.is_empty()).then(|| token.to_string())
}

/// `findHuggingFaceToken` (`huggingface.ts:46-61`): `HF_TOKEN` (trimmed), else the first usable
/// token file of `$HF_TOKEN_PATH`, `$HF_HOME/token`, `$XDG_CACHE_HOME/huggingface/token` and
/// `~/.cache/huggingface/token`, de-duplicated and read in that order. The home directory is the
/// process's, as `os.homedir()` is (`huggingface.ts:54`), not one taken from `env`.
///
/// The `std::env::home_dir()` call is proven in a RE-EXECUTED child with `HOME` set
/// (`crate::tests::huggingface::home_dir_is_the_process_home`, EXT-099b), not by mutating this
/// process: `std::env::set_var` is `unsafe` and this crate is `#![forbid(unsafe_code)]`
/// (`lib.rs:31`).
pub async fn find_huggingface_token(env: &BTreeMap<String, String>) -> Option<String> {
    find_huggingface_token_with_home(env, std::env::home_dir().as_deref()).await
}

/// The token files to try, in order, with duplicates dropped (`huggingface.ts:50-55` builds
/// the array, `:56` is `for (const path of new Set(paths))`).
///
/// This is its own function so the de-duplication is OBSERVABLE (EXT-099a). Reading the same file
/// twice yields the same token, so the `Set` changes no answer `find_huggingface_token_with_home`
/// can give — its only effect is one fewer [`read_token`] call when two of the four candidates
/// name one file, which happens whenever `HF_TOKEN_PATH` is `$HF_HOME/token` or `HF_HOME` is
/// `$XDG_CACHE_HOME/huggingface`. Returning the list makes that effect something a test can
/// assert, so dropping the `contains` check below turns
/// [`crate::tests::huggingface::token_file_candidates_drop_duplicates_keeping_the_first_position`]
/// red instead of passing silently.
pub(crate) fn token_file_candidates(
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let non_empty = |name: &str| env.get(name).filter(|value| !value.is_empty());
    let candidates = [
        non_empty("HF_TOKEN_PATH").map(PathBuf::from),
        non_empty("HF_HOME").map(|home| Path::new(home).join("token")),
        non_empty("XDG_CACHE_HOME").map(|cache| Path::new(cache).join("huggingface").join("token")),
        home.map(|home| home.join(".cache").join("huggingface").join("token")),
    ];
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in candidates.into_iter().flatten() {
        // `new Set(paths)` (`huggingface.ts:56`): the FIRST occurrence keeps its position, which is
        // what JS `Set` insertion order gives.
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// [`find_huggingface_token`] with the home directory given explicitly (the seam tests use,
/// because `os.homedir()` has no injected equivalent upstream).
pub async fn find_huggingface_token_with_home(
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
) -> Option<String> {
    if let Some(token) = env.get("HF_TOKEN").map(|token| js_trim(token))
        && !token.is_empty()
    {
        return Some(token.to_string());
    }

    for path in token_file_candidates(env, home) {
        if let Some(token) = read_token(&path).await {
            return Some(token);
        }
    }
    None
}

// --------------------------------------------------------------------------------- quantizations --

/// The quantization a GGUF file name belongs to, or `None` (`huggingface.ts:133-135`): the
/// directory part is dropped by the caller, `.gguf` and a `-00001-of-00002` shard suffix are
/// stripped, and the stem must end in a known quantization token after `-`, `_`, `.` or the start.
fn quantization_of_stem(filename: &str) -> Option<String> {
    let stem_end = filename
        .char_indices()
        .rev()
        .nth(4)
        .map_or(0, |(index, _)| index);
    let stem = filename.get(..stem_end).unwrap_or_default();
    let stem = SHARD_SUFFIX.replace(stem, "");
    let captures = QUANTIZATION.captures(&stem)?;
    Some(captures.get(1)?.as_str().to_uppercase())
}

/// The quantization of one `rfilename` of a repository's file list (`huggingface.ts:130-135`):
/// `None` unless it is a `.gguf` file, is not a multimodal projector (`mmproj*`), and names a
/// quantization.
#[must_use]
pub fn quantization_of_file(rfilename: &str) -> Option<String> {
    if !rfilename.to_lowercase().ends_with(".gguf") {
        return None;
    }
    let filename = rfilename.rsplit('/').next().unwrap_or(rfilename);
    if filename.to_lowercase().starts_with("mmproj") {
        return None;
    }
    quantization_of_stem(filename)
}

/// ICU root-collation weight of a character of a quantization name, for
/// `left.name.localeCompare(right.name)` (`huggingface.ts:149`). Names are upper-case ASCII
/// letters, digits, `_` and `-`; the collation puts `_` before `-` before digits before letters,
/// where code-point order would put `_` after the letters.
fn collation_weight(c: char) -> (u8, char) {
    match c {
        '_' => (0, c),
        '-' => (1, c),
        c if c.is_ascii_digit() => (2, c),
        c => (3, c),
    }
}

fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    left.chars()
        .map(collation_weight)
        .cmp(right.chars().map(collation_weight))
}

/// The comparator of `huggingface.ts:144-151`.
fn compare_quantizations(
    left: &HuggingFaceQuantization,
    right: &HuggingFaceQuantization,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if left.name == "Q4_K_M" {
        return Ordering::Less;
    }
    if right.name == "Q4_K_M" {
        return Ordering::Greater;
    }
    // `Number.MAX_SAFE_INTEGER` stands in for an unknown size.
    const UNKNOWN: f64 = 9_007_199_254_740_991.0;
    let difference = left.size.unwrap_or(UNKNOWN) - right.size.unwrap_or(UNKNOWN);
    if difference < 0.0 {
        Ordering::Less
    } else if difference > 0.0 {
        Ordering::Greater
    } else {
        locale_compare(&left.name, &right.name)
    }
}

// ---------------------------------------------------------------------------------------- client --

/// HTTP client of the Hugging Face API (`HuggingFaceClient`, `huggingface.ts:63-158`).
#[derive(Clone)]
pub struct HuggingFaceClient {
    token: Option<String>,
    base_url: String,
    http: reqwest::Client,
    timeout: Duration,
}

impl std::fmt::Debug for HuggingFaceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HuggingFaceClient")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl HuggingFaceClient {
    /// `new HuggingFaceClient(token?, baseUrl = DEFAULT_HUGGING_FACE_URL)`
    /// (`huggingface.ts:67-70`). Trailing slashes of the base URL are dropped; an empty token is
    /// no token (`if (this.token)`, `:74`).
    ///
    /// The HTTP client resolves its proxy through [`build_client_for_target`], the same path the
    /// llama.cpp client and the classifier use (`env` is the provider-scoped overlay), as pi's
    /// global `EnvHttpProxyAgent` dispatcher does for its `fetch` (`core/http-dispatcher.ts:87-100`).
    ///
    /// # Errors
    ///
    /// A transport error when the proxy setting is unusable or the HTTP stack cannot be
    /// initialised.
    pub async fn new(
        token: Option<String>,
        base_url: Option<&str>,
        env: Option<&ProviderEnv>,
    ) -> Result<Self, LlamaError> {
        let target = base_url
            .unwrap_or(DEFAULT_HUGGING_FACE_URL)
            .trim_end_matches('/');
        let http = build_client_for_target(target, &EnvAuthContext, env, None)
            .await
            .map_err(|error| LlamaError::transport(&error))?;
        Ok(Self::with_http_client(token, base_url, http))
    }

    /// [`Self::new`] over a caller-supplied `reqwest::Client` (proxy, TLS or test configuration).
    #[must_use]
    pub fn with_http_client(
        token: Option<String>,
        base_url: Option<&str>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            token: token.filter(|token| !token.is_empty()),
            base_url: base_url
                .unwrap_or(DEFAULT_HUGGING_FACE_URL)
                .trim_end_matches('/')
                .to_string(),
            http,
            timeout: REQUEST_TIMEOUT,
        }
    }

    /// Replace the 15 s per-request timeout (`huggingface.ts:75`); chiefly for tests.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The base URL without trailing slashes (`baseUrl`, `huggingface.ts:65`).
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `request` (`huggingface.ts:72-98`): one JSON `GET` with the 15 s timeout combined with the
    /// caller's cancellation. A body that is not JSON reads as no payload (`:80-85`). HTTP 429
    /// fails with the retry delay from `retry-after`, else the `t=` of the `ratelimit` header
    /// (`:88-94`); any other non-2xx status fails with the payload's `error` or
    /// `Hugging Face returned HTTP <n>` (`:87`, `:95`).
    async fn request(
        &self,
        path: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Value>, LlamaError> {
        let work = async {
            let mut request = self.http.get(format!("{}{path}", self.base_url));
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(LlamaError::from_reqwest)?;
            let status = response.status();
            let headers = response.headers().clone();
            let payload = match read_capped_body(response, MAX_BODY_BYTES).await {
                BodyRead::Complete(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
                BodyRead::Unreadable => None,
                BodyRead::TooLarge => {
                    return Err(LlamaError::Message(format!(
                        "Hugging Face response exceeds {MAX_BODY_BYTES} bytes"
                    )));
                }
            };
            if !status.is_success() {
                if status.as_u16() == 429 {
                    let delay = header_text(&headers, RETRY_AFTER.as_str())
                        .as_deref()
                        .and_then(js_truthy_number)
                        .or_else(|| {
                            parse_rate_limit_delay(header_text(&headers, "ratelimit").as_deref())
                                .filter(|delay| *delay != 0.0)
                        });
                    return Err(LlamaError::Message(match delay {
                        Some(delay) => format!(
                            "Hugging Face rate limit reached; retry in {}s",
                            js_number_text(delay)
                        ),
                        None => "Hugging Face rate limit reached".to_string(),
                    }));
                }
                return Err(LlamaError::Message(payload_error(
                    payload.as_ref(),
                    &format!("Hugging Face returned HTTP {}", status.as_u16()),
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

    /// `search` (`huggingface.ts:100-116`): `GET /api/models?search=<query>&filter=gguf&
    /// sort=downloads&direction=-1&limit=20`. Entries without a string `id` are dropped.
    ///
    /// # Errors
    ///
    /// `Hugging Face returned invalid search results` when the body is not an array, the request
    /// errors of [`Self::request`], or cancellation.
    pub async fn search(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<HuggingFaceModel>, LlamaError> {
        let params = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("search", query)
            .append_pair("filter", "gguf")
            .append_pair("sort", "downloads")
            .append_pair("direction", "-1")
            .append_pair("limit", "20")
            .finish();
        let payload = self
            .request(&format!("/api/models?{params}"), cancel)
            .await?;
        let Some(Value::Array(entries)) = payload else {
            return Err(LlamaError::Message(
                "Hugging Face returned invalid search results".to_string(),
            ));
        };
        Ok(entries
            .iter()
            .filter_map(|entry| {
                let id = entry.get("id")?.as_str()?;
                Some(HuggingFaceModel {
                    id: id.to_string(),
                    downloads: entry
                        .get("downloads")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                })
            })
            .collect())
    }

    /// `details` (`huggingface.ts:118-157`): `GET /api/models/<owner>/<repo>?blobs=true`, reading
    /// `id`, `gated` and the `siblings` file list into per-quantization sizes.
    ///
    /// # Errors
    ///
    /// `Hugging Face returned invalid model details` when the body is not an object, the request
    /// errors of [`Self::request`], or cancellation.
    pub async fn details(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<HuggingFaceModelDetails, LlamaError> {
        let encoded_id = id
            .split('/')
            .map(encode_uri_component)
            .collect::<Vec<_>>()
            .join("/");
        let payload = self
            .request(&format!("/api/models/{encoded_id}?blobs=true"), cancel)
            .await?;
        // `typeof payload !== "object" || payload === null` (`:121`): an array passes.
        let Some(model @ (Value::Object(_) | Value::Array(_))) = payload else {
            return Err(LlamaError::Message(
                "Hugging Face returned invalid model details".to_string(),
            ));
        };

        // name -> (total bytes, every file had a size); insertion order is irrelevant because the
        // list is sorted below.
        let mut sizes: BTreeMap<String, (f64, bool)> = BTreeMap::new();
        if let Some(Value::Array(siblings)) = model.get("siblings") {
            for file in siblings {
                let Some(rfilename) = file.get("rfilename").and_then(Value::as_str) else {
                    continue;
                };
                let Some(quantization) = quantization_of_file(rfilename) else {
                    continue;
                };
                let current = sizes.entry(quantization).or_insert((0.0, true));
                match file.get("size").and_then(Value::as_f64) {
                    Some(size) => current.0 += size,
                    None => current.1 = false,
                }
            }
        }
        let mut quantizations: Vec<HuggingFaceQuantization> = sizes
            .into_iter()
            .map(|(name, (total, complete))| HuggingFaceQuantization {
                name,
                size: complete.then_some(total),
            })
            .collect();
        quantizations.sort_by(compare_quantizations);

        Ok(HuggingFaceModelDetails {
            id: model
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string(),
            gated: match model.get("gated").and_then(Value::as_str) {
                Some("auto") => HuggingFaceGated::Auto,
                Some("manual") => HuggingFaceGated::Manual,
                _ => HuggingFaceGated::Open,
            },
            quantizations,
        })
    }
}
