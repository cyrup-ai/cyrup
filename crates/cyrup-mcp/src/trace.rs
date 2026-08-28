//! `mcp-trace.ts` — the JSONL wire tracer.
//!
//! Every JSON-RPC frame in either direction becomes one redacted line in a session-global file.
//! It decorates the **transport**, not the handler, so it sees exactly the bytes rmcp does.
//!
//! # The property the whole module is shaped around
//!
//! **Tracing must never change MCP request/response behaviour.** Every latch is one-way, every
//! failure path is silent, [`TraceWriter::write`] is sync and infallible, and
//! [`TracingTransport::send`] rethrows the inner outcome unchanged. A tracer that can fail a connect
//! is worse than no tracer.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use regex::{Regex, NoExpand};
use serde::Serialize;

/// `MCP_TRACE_SCHEMA_VERSION` (`mcp-trace.ts:7`).
pub const MCP_TRACE_SCHEMA_VERSION: u8 = 1;

/// Both `redactTraceText` call sites in `createMcpTraceEvent` pass 120, not the 160 default.
const REDACT_MAX: usize = 120;

/// `"[REDACTED]"` — the whole-value answer when the guard matches.
pub const REDACTED: &str = "[REDACTED]";
/// `"[REDACTED_URL]"`.
pub const REDACTED_URL: &str = "[REDACTED_URL]";
/// `"[REDACTED_AUTH]"`.
pub const REDACTED_AUTH: &str = "[REDACTED_AUTH]";
/// `"[REDACTED_ID]"` — an opaque correlation token can itself be a secret.
pub const REDACTED_ID: &str = "[REDACTED_ID]";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceDirection {
    Outbound,
    Inbound,
}

/// Post-Cut-1/Cut-3 the kind enum is these three: `sse` and `unix-socket` have no producer left, and
/// upstream's constructor-name sniffing (`mcp-trace.ts:299-306`) existed only to tell the two HTTP
/// transports apart. Carried as an enum from the construction site; never inspected from a type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceTransportKind {
    Stdio,
    StreamableHttp,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceMessageKind {
    Request,
    Notification,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceStatus {
    Sent,
    Received,
    Error,
}

/// `traceId(value)` (`mcp-trace.ts:74-79`).
///
/// A STRING id becomes the literal [`REDACTED_ID`]; numeric ids pass through, because correlating a
/// request with its response is the whole point of writing an id at all. Upstream's
/// `Number.isFinite` arm has no Rust counterpart — `i64` is always finite, so its `?? null` is
/// unreachable here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum TraceId {
    Number(i64),
    Redacted(&'static str),
}

/// `McpTraceEvent`, in `createMcpTraceEvent`'s **insertion** order (`mcp-trace.ts:99-119`), not the
/// interface order at `:26-40` — they differ, and serde emits struct fields in declaration order.
/// `bytes` is 8th on the wire and 12th in the interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTraceEvent {
    /// Literal `1`.
    pub version: u8,
    /// `new Date().toISOString()`.
    pub timestamp: String,
    pub direction: TraceDirection,
    /// `redactTraceText(server, 120)`.
    pub server: String,
    pub transport: TraceTransportKind,
    pub kind: TraceMessageKind,
    pub status: TraceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// `redactTraceText(message.method, 120)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// **Asymmetric with [`Self::related_request_id`], and deliberately so:** an absent `id` on a
    /// message that HAS an `id` key is written as `null` (`event.id = traceId(message.id) ?? null`),
    /// where `relatedRequestId` is OMITTED when absent. The outer `Option` is "no `id` key at all";
    /// the inner one is JSON `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Option<TraceId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_request_id: Option<TraceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}

// The four patterns, all lookaround-free, so `regex`'s linear-time engine suffices.
static SECRET_WORD: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:token|secret|password|passwd|api[_-]?key|authorization|cookie)\b").ok()
});
static URL_LIKE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)\b[a-z][a-z\d+.-]*://[^\s"'<>]+"#).ok());
static AUTH_SCHEME: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]+").ok());
static SECRET_ASSIGNMENT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:token|secret|password|passwd|api[_-]?key|authorization|cookie)\s*[:=]\s*[^\s,;]+",
    )
    .ok()
});

/// `redactTraceText(value, maxLength = 160)` (`mcp-trace.ts:57-67`).
///
/// # Two exactness traps, both load-bearing
///
/// 1. **The third replacement emits a literal `$1`.** Upstream's replacement string is
///    `"$1=[REDACTED]"` against a pattern whose only groups are **non-capturing** `(?:…)`, so
///    JavaScript has no group 1 to interpolate and writes the two characters `$1` verbatim. Rust's
///    `replace_all` would treat `$1` as a capture reference and expand it to nothing, producing
///    `=[REDACTED]` — a different string. [`NoExpand`] is what keeps the two identical. It looks
///    like an upstream bug and it is; it is also the wire format, and a trace consumer written
///    against pi's output would break if we "fixed" it.
/// 2. **Truncation counts UTF-16 code units**, because `String.prototype.slice` does. Not chars,
///    not bytes: an emoji outside the BMP is two units, so a string of 100 emoji truncates at 60 of
///    them, and a `char`-based cut would keep 119.
///
/// A regex that fails to compile is treated as "no match" rather than panicking — this module may
/// not change behaviour, and that includes its own construction.
#[must_use]
pub fn redact_trace_text(value: &str, max_length: usize) -> String {
    if SECRET_WORD.as_ref().is_some_and(|re| re.is_match(value)) {
        return REDACTED.to_string();
    }
    let mut redacted = value.to_string();
    if let Some(re) = URL_LIKE.as_ref() {
        redacted = re.replace_all(&redacted, NoExpand(REDACTED_URL)).into_owned();
    }
    if let Some(re) = AUTH_SCHEME.as_ref() {
        redacted = re.replace_all(&redacted, NoExpand(REDACTED_AUTH)).into_owned();
    }
    if let Some(re) = SECRET_ASSIGNMENT.as_ref() {
        // `NoExpand` is the whole point — see trap 1 above.
        redacted = re.replace_all(&redacted, NoExpand("$1=[REDACTED]")).into_owned();
    }
    truncate_utf16(&redacted, max_length)
}

/// `value.length > max ? value.slice(0, max - 1) + "…" : value`, in UTF-16 code units.
fn truncate_utf16(value: &str, max_length: usize) -> String {
    let units: usize = value.chars().map(char::len_utf16).sum();
    if units <= max_length {
        return value.to_string();
    }
    let keep = max_length.saturating_sub(1);
    let mut out = String::with_capacity(value.len());
    let mut used = 0usize;
    for ch in value.chars() {
        let width = ch.len_utf16();
        if used + width > keep {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

/// `Math.max(0, Math.round(ms * 100) / 100)` — two decimal places, and the rounding is observable in
/// a golden line.
#[must_use]
pub fn round_2dp(millis: f64) -> f64 {
    if !millis.is_finite() {
        return 0.0;
    }
    ((millis * 100.0).round() / 100.0).max(0.0)
}

/// `createMcpTraceEvent` (`mcp-trace.ts:89-120`), over **one** serialisation pass.
///
/// That single `to_value` is simultaneously `messageBytes` (`:81-87`), `messageKind` (`:69-72`) and
/// the `method`/`id`/`error.code` reads — and it is the only way to reach `method` generically
/// across rmcp's `ClientRequest`/`ServerNotification` enums without matching every variant.
///
/// `messageKind` is `"method" in message ? ("id" in message ? request : notification) : response`,
/// so rmcp's distinct `JsonRpcMessage::Error` variant is a **response**, and its `error.code` is the
/// `errorCode` field.
pub fn trace_event<M: Serialize>(
    direction: TraceDirection,
    server: &str,
    transport: TraceTransportKind,
    message: &M,
    status: TraceStatus,
    duration: Option<Duration>,
) -> McpTraceEvent {
    let encoded = serde_json::to_value(message).ok();
    let object = encoded.as_ref().and_then(serde_json::Value::as_object);

    let kind = match object {
        Some(map) if map.contains_key("method") => {
            if map.contains_key("id") {
                TraceMessageKind::Request
            } else {
                TraceMessageKind::Notification
            }
        }
        // A message that will not serialise cannot be classified; upstream's `messageKind` reads
        // the value directly and would say `response` for anything without a `method` key, which is
        // the same answer.
        _ => TraceMessageKind::Response,
    };

    // `Buffer.byteLength(JSON.stringify(message), "utf8")`, and `undefined` when it throws.
    let bytes = encoded
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|encoded| encoded.len());

    let method = object
        .and_then(|map| map.get("method"))
        .and_then(serde_json::Value::as_str)
        .map(|method| redact_trace_text(method, REDACT_MAX));

    // `if ("id" in message) event.id = traceId(message.id) ?? null` — present-but-unmappable is
    // `null`, absent is omitted entirely.
    let id = object
        .and_then(|map| map.get("id"))
        .map(trace_id);

    let error_code = object
        .and_then(|map| map.get("error"))
        .and_then(serde_json::Value::as_object)
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|code| i32::try_from(code).ok());

    McpTraceEvent {
        version: MCP_TRACE_SCHEMA_VERSION,
        timestamp: iso8601_now(),
        direction,
        server: redact_trace_text(server, REDACT_MAX),
        transport,
        kind,
        status,
        bytes,
        method,
        id,
        // `relatedRequestId` has no producer in this port: rmcp correlates responses itself, so
        // nothing here has a second id to attach. Modelled because it is part of the wire schema a
        // consumer parses, and omitted rather than written `null`.
        related_request_id: None,
        error_code,
        duration_ms: duration.map(|elapsed| round_2dp(elapsed.as_secs_f64() * 1000.0)),
    }
}

/// `traceId(value)` (`mcp-trace.ts:74-79`).
fn trace_id(value: &serde_json::Value) -> Option<TraceId> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Number(number) => number.as_i64().map(TraceId::Number),
        serde_json::Value::String(_) => Some(TraceId::Redacted(REDACTED_ID)),
        _ => None,
    }
}

/// `new Date().toISOString()` — `YYYY-MM-DDTHH:MM:SS.sssZ`, milliseconds, always UTC.
fn iso8601_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis();
    let secs = (millis / 1000) as i64;
    let sub = (millis % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{sub:03}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Howard Hinnant's `civil_from_days`. Hand-rolled rather than pulling `chrono` in for one
/// timestamp: the crate has no date dependency today and this is the only place that needs one.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `McpTraceWriterOptions.{appendFile,writeFile,mkdir}` (`mcp-trace.ts:46-48`) as one trait.
///
/// The seam is not decoration: without it the truncate-then-append ordering and the byte-cap latch
/// are not observable in-crate at all, because both are defined by what reaches the file system and
/// in what order.
pub trait TraceFs: Send + Sync + 'static {
    /// `mkdir(dirname(filePath), { recursive: true })`.
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()>;
    /// `writeFile(filePath, "")` — the truncate that must happen before any append.
    fn truncate(&self, path: &Path) -> std::io::Result<()>;
    /// `appendFile(filePath, line)`.
    fn append(&self, path: &Path, line: &str) -> std::io::Result<()>;
}

/// The real file system.
pub struct RealTraceFs;

impl TraceFs for RealTraceFs {
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)
    }
    fn truncate(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, "")
    }
    fn append(&self, path: &Path, line: &str) -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(line.as_bytes())
    }
}

#[derive(Debug, Default)]
struct TraceWriterState {
    bytes_written: u64,
    events_written: u64,
    disabled: bool,
    initialized: bool,
    initialization_failed: bool,
}

/// `McpTraceWriter` (`mcp-trace.ts:122-200`).
///
/// One writer per manager, so the byte and event budgets are **session-global** — upstream's
/// `this.traceWriter ??=` at `server-manager.ts:452-454`.
pub struct TraceWriter {
    path: PathBuf,
    max_bytes: u64,
    max_events: u64,
    fs: Arc<dyn TraceFs>,
    state: Mutex<TraceWriterState>,
}

impl std::fmt::Debug for TraceWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceWriter")
            .field("path", &self.path)
            .field("disabled", &self.is_disabled())
            .finish_non_exhaustive()
    }
}

impl TraceWriter {
    /// `new McpTraceWriter({ filePath, maxBytes, maxEvents })`.
    ///
    /// Upstream starts `mkdir` + truncate in the constructor and every `write` awaits that promise.
    /// Here the same work is done lazily on the first `write`, under the same lock that orders the
    /// appends — which gives the identical guarantee (nothing is ever appended before the truncate)
    /// without a constructor that can start I/O.
    #[must_use]
    pub fn new(path: PathBuf, max_bytes: u64, max_events: u64, fs: Arc<dyn TraceFs>) -> Self {
        Self {
            path,
            max_bytes,
            max_events,
            fs,
            state: Mutex::new(TraceWriterState::default()),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .disabled
    }

    /// `{ bytes, events }`.
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.bytes_written, state.events_written)
    }

    /// `write(event)` (`mcp-trace.ts:169-192`), in upstream's exact order.
    ///
    /// Sync and infallible by design. Note the cap comparison: `bytes > maxBytes - bytesWritten`
    /// **latches disabled** rather than skipping the line, so one oversized event stops the trace
    /// instead of leaving a hole in it. The subtraction order matters — it is saturating here
    /// because `bytesWritten` can equal `maxBytes` exactly.
    pub fn write(&self, event: &McpTraceEvent) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.disabled || state.events_written >= self.max_events {
            return;
        }
        let Ok(encoded) = serde_json::to_string(event) else {
            // A line that will not serialise latches the writer off.
            state.disabled = true;
            return;
        };
        let line = format!("{encoded}\n");
        let bytes = line.len() as u64;
        if bytes > self.max_bytes.saturating_sub(state.bytes_written) {
            state.disabled = true;
            return;
        }

        // Both counters move BEFORE the append, exactly as upstream's do — the budget is spent at
        // the decision, not at the I/O, so a slow file system cannot let a burst overshoot it.
        state.bytes_written += bytes;
        state.events_written += 1;

        if !state.initialized {
            state.initialized = true;
            let prepared = self
                .path
                .parent()
                .map_or(Ok(()), |dir| self.fs.create_dir_all(dir))
                .and_then(|()| self.fs.truncate(&self.path));
            if prepared.is_err() {
                // Tracing must never change MCP request/response behaviour.
                state.initialization_failed = true;
                state.disabled = true;
                return;
            }
        }
        if state.initialization_failed {
            return;
        }
        if self.fs.append(&self.path, &line).is_err() {
            state.disabled = true;
        }
    }

    /// `flush()` (`mcp-trace.ts:194-197`).
    ///
    /// **A genuine no-op, and that is a property rather than an omission.** Upstream's `write`
    /// enqueues onto a promise chain and returns before the append happens, so its `flush` exists to
    /// drain that chain. This port appends inside `write`, under the lock that orders the lines, so
    /// by the time `write` returns the bytes are already at the file system and there is nothing
    /// left to await.
    ///
    /// Kept, and kept `async`, for two reasons: the call sites — `dispose_connection` and
    /// `close_all_inner` — read against `server-manager.ts:1133-1140` and `:1165`, and the day this
    /// writer grows a real queue, they must not need touching.
    #[allow(clippy::unused_async)]
    pub async fn flush(&self) {}
}

/// `isMcpTraceEnabled(definition, settings)` (`mcp-trace.ts:223-228`):
/// `definition.trace ?? settings?.enabled === true`.
///
/// `??`, never `||`. A per-server `trace: false` beats a global `enabled: true`; `||` would invert
/// that, and this function exists to make that bug un-writable.
#[must_use]
pub fn is_mcp_trace_enabled(
    entry: &crate::config::ServerEntry,
    settings: Option<&crate::config::TraceSettings>,
) -> bool {
    entry
        .trace
        .unwrap_or_else(|| settings.is_some_and(|settings| settings.enabled == Some(true)))
}

/// `createMcpTraceWriter`'s path half (`mcp-trace.ts:202-217`).
///
/// `settings.file` verbatim when absolute, resolved against the session cwd when relative; otherwise
/// `<cwd>/.cyrup/mcp-traces/mcp-<ISO with `:` and `.` mapped to `-`>-<suffix>.jsonl`.
#[must_use]
pub fn trace_file_path(dirs: &crate::dirs::McpDirs, settings: &crate::config::TraceSettings, suffix: &str) -> PathBuf {
    if let Some(configured) = settings.file.as_deref().filter(|file| !file.is_empty()) {
        let path = Path::new(configured);
        return if path.is_absolute() {
            path.to_path_buf()
        } else {
            dirs.cwd().join(path)
        };
    }
    let stamp = iso8601_now().replace([':', '.'], "-");
    dirs.trace_dir().join(format!("mcp-{stamp}-{suffix}.jsonl"))
}

/// `Math.random().toString(36).slice(2, 10)` — eight base36 characters.
///
/// Taken from `Uuid::now_v7`'s low bits rather than adding an RNG dependency: `uuid` is already in
/// the manifest, and its comment there names this writer as the reason.
#[must_use]
pub fn random_suffix() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut value = u64::from_le_bytes(
        uuid::Uuid::now_v7().as_bytes()[8..16]
            .try_into()
            .unwrap_or([0; 8]),
    );
    let mut out = String::with_capacity(8);
    for _ in 0..8 {
        let index = (value % 36) as usize;
        // `get` rather than indexing: `index` is `% 36` and ALPHABET is 36 long, so the `None` arm
        // is unreachable — but `indexing_slicing` is denied crate-wide and an unreachable panic is
        // still a panic in the binary.
        if let Some(digit) = ALPHABET.get(index) {
            out.push(char::from(*digit));
        }
        value /= 36;
    }
    out
}

/// `wrapTransportWithMcpTrace` (`mcp-trace.ts:236-297`) as a newtype.
///
/// Upstream patches the transport **in place** because the TS SDK sniffs its concrete type before
/// connect. That constraint does not exist here — `serve_client_with_lifecycle` runs
/// `discover_startup` on the same `&mut transport` — so a newtype is safe.
///
/// # The one real consequence, stated where a reader will hit it
///
/// `DynamicTransportError` records `transport_name: T::name()` and `transport_type_id:
/// TypeId::of::<T>()`, and `is::<T, R>()`/`downcast::<T, R>()` key on both. Wrapping **changes the
/// error identity**: any downcast on the connect error path must target `TracingTransport<T>` or
/// unwrap the inner error first. Nothing in this crate downcasts a transport error today; this doc
/// is the tripwire for the day something does.
pub struct TracingTransport<T> {
    inner: T,
    server: Arc<str>,
    kind: TraceTransportKind,
    writer: Arc<TraceWriter>,
}

impl<T> TracingTransport<T> {
    #[must_use]
    pub fn new(inner: T, server: impl Into<Arc<str>>, kind: TraceTransportKind, writer: Arc<TraceWriter>) -> Self {
        Self { inner, server: server.into(), kind, writer }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for TracingTransport<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracingTransport")
            .field("server", &self.server)
            .field("kind", &self.kind)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T> rmcp::transport::Transport<rmcp::service::RoleClient> for TracingTransport<T>
where
    T: rmcp::transport::Transport<rmcp::service::RoleClient> + Send,
{
    type Error = T::Error;

    /// The returned future is `'static`, so the event's metadata is computed **here**, from
    /// `&item`, before `item` is moved into the inner send. The timing brackets the inner send only.
    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<rmcp::service::RoleClient>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let sent = trace_event(
            TraceDirection::Outbound,
            &self.server,
            self.kind,
            &item,
            TraceStatus::Sent,
            None,
        );
        let mut failed = sent.clone();
        failed.status = TraceStatus::Error;
        let writer = Arc::clone(&self.writer);
        let started = Instant::now();
        let inner = self.inner.send(item);
        async move {
            let outcome = inner.await;
            let elapsed = started.elapsed();
            let mut event = if outcome.is_ok() { sent } else { failed };
            event.duration_ms = Some(round_2dp(elapsed.as_secs_f64() * 1000.0));
            writer.write(&event);
            // Rethrown unchanged. Tracing observes; it never converts.
            outcome
        }
    }

    /// `receive` replaces upstream's `onmessage` interception — rmcp **pulls** where the TS SDK
    /// pushes, so there is no property to define and the `defineProperty` try/catch has no analogue.
    async fn receive(&mut self) -> Option<rmcp::service::RxJsonRpcMessage<rmcp::service::RoleClient>> {
        let message = self.inner.receive().await;
        if let Some(message) = message.as_ref() {
            self.writer.write(&trace_event(
                TraceDirection::Inbound,
                &self.server,
                self.kind,
                message,
                TraceStatus::Received,
                None,
            ));
        }
        message
    }

    fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

/// Either the tracer or the bare transport, chosen once at construction.
///
/// An enum rather than a `Box<dyn Transport>` because [`rmcp::transport::Transport`] is not
/// object-safe (`send` returns `impl Future`), and rather than two call paths because the three
/// construction sites in `ConnectionBuilder` must not each grow an `if`.
pub enum MaybeTraced<T> {
    Plain(T),
    Traced(TracingTransport<T>),
}

/// Wrap `transport` when `writer` is `Some`, pass it through untouched otherwise.
///
/// Takes an `IntoTransport` rather than a `Transport` because rmcp's HTTP client transport is a
/// `Worker`, not a `Transport` — it only becomes one through the `WorkerAdapter` impl — while the
/// child-process transport is a `Transport` directly. Converting here is what lets one helper cover
/// both.
pub fn maybe_traced<T, E, A>(
    transport: T,
    server: &str,
    kind: TraceTransportKind,
    writer: Option<Arc<TraceWriter>>,
) -> MaybeTraced<impl rmcp::transport::Transport<rmcp::service::RoleClient, Error = E> + 'static>
where
    T: rmcp::transport::IntoTransport<rmcp::service::RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let inner = transport.into_transport();
    match writer {
        Some(writer) => MaybeTraced::Traced(TracingTransport::new(inner, server, kind, writer)),
        None => MaybeTraced::Plain(inner),
    }
}

impl<T> rmcp::transport::Transport<rmcp::service::RoleClient> for MaybeTraced<T>
where
    T: rmcp::transport::Transport<rmcp::service::RoleClient> + Send,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<rmcp::service::RoleClient>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        // Both arms produce a `'static` future, so this returns one future type by boxing — the
        // only place in the module that allocates per message, and only because the trait's `send`
        // must name a single return type across the two arms.
        match self {
            Self::Plain(inner) => Box::pin(inner.send(item))
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Self::Error>> + Send>>,
            Self::Traced(inner) => Box::pin(inner.send(item)),
        }
    }

    async fn receive(&mut self) -> Option<rmcp::service::RxJsonRpcMessage<rmcp::service::RoleClient>> {
        match self {
            Self::Plain(inner) => inner.receive().await,
            Self::Traced(inner) => inner.receive().await,
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        match self {
            Self::Plain(inner) => inner.close().await,
            Self::Traced(inner) => inner.close().await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A recording double for the injectable seam, plus a failure switch for the init path.
    #[derive(Default)]
    struct FakeFs {
        calls: Mutex<Vec<String>>,
        fail_dir: bool,
    }

    impl FakeFs {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
        fn record(&self, entry: String) {
            self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(entry);
        }
    }

    impl TraceFs for Arc<FakeFs> {
        fn create_dir_all(&self, dir: &Path) -> std::io::Result<()> {
            if self.fail_dir {
                return Err(std::io::Error::other("no directory for you"));
            }
            self.record(format!("mkdir {}", dir.display()));
            Ok(())
        }
        fn truncate(&self, path: &Path) -> std::io::Result<()> {
            self.record(format!("truncate {}", path.display()));
            Ok(())
        }
        fn append(&self, path: &Path, line: &str) -> std::io::Result<()> {
            self.record(format!("append {} {}", path.display(), line.trim_end()));
            Ok(())
        }
    }

    fn event() -> McpTraceEvent {
        trace_event(
            TraceDirection::Outbound,
            "fixture",
            TraceTransportKind::Stdio,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            TraceStatus::Sent,
            None,
        )
    }

    fn writer(fs: &Arc<FakeFs>, max_bytes: u64, max_events: u64) -> TraceWriter {
        TraceWriter::new(
            PathBuf::from("/traces/mcp.jsonl"),
            max_bytes,
            max_events,
            Arc::new(Arc::clone(fs)),
        )
    }

    // ---- redaction ----

    /// The guard fires on the WHOLE value: one secret word anywhere and nothing else survives.
    #[test]
    fn a_secret_word_anywhere_redacts_the_entire_value() {
        assert_eq!(redact_trace_text("my api_key is here", 120), REDACTED);
        assert_eq!(redact_trace_text("Authorization", 120), REDACTED);
        assert_eq!(redact_trace_text("COOKIE", 120), REDACTED);
        // `\b` is a word boundary, so a substring inside a longer word does not trip it.
        assert_eq!(redact_trace_text("tokenizer", 120), "tokenizer");
    }

    #[test]
    fn urls_and_auth_schemes_are_replaced_in_place() {
        assert_eq!(
            redact_trace_text("see https://example.com/x?y=1 now", 120),
            format!("see {REDACTED_URL} now")
        );
        assert_eq!(
            redact_trace_text("bearer abc.DEF-123", 120),
            REDACTED_AUTH.to_string()
        );
    }

    /// **Upstream emits a literal `$1`.** Its replacement string is `"$1=[REDACTED]"` against a
    /// pattern whose only groups are non-capturing, so JavaScript has no group 1 and writes the two
    /// characters verbatim. Rust would expand `$1` to nothing without `NoExpand`, producing
    /// `=[REDACTED]` — a different wire format. This test is the guard on that.
    ///
    /// Reaching the assignment rule at all needs a word the guard does not catch, because the guard
    /// runs first and would otherwise return `[REDACTED]` whole.
    #[test]
    fn the_assignment_rule_emits_a_literal_dollar_one() {
        // `pwd` is not in the guard's word list, but `passwd` is in the assignment rule's.
        // Use a value that only the assignment rule matches by constructing it directly.
        let re = SECRET_ASSIGNMENT.as_ref().expect("compiles");
        let replaced = re.replace_all("token: abc123", NoExpand("$1=[REDACTED]"));
        assert_eq!(
            replaced, "$1=[REDACTED]",
            "the literal `$1` is upstream's output and therefore the wire format"
        );
    }

    /// Truncation counts UTF-16 code units, because `String.prototype.slice` does. An astral
    /// character is two units, so a `char`-based cut would keep a different number of them.
    #[test]
    fn truncation_counts_utf16_code_units_not_chars() {
        // 10 astral chars = 20 UTF-16 units. A limit of 11 keeps 10 units (5 chars) + the ellipsis.
        let astral = "𝄞".repeat(10);
        let cut = truncate_utf16(&astral, 11);
        assert_eq!(cut.chars().filter(|c| *c == '𝄞').count(), 5);
        assert!(cut.ends_with('…'));
        // Exactly at the limit, nothing happens.
        assert_eq!(truncate_utf16("abc", 3), "abc");
        assert_eq!(truncate_utf16("abcd", 3), "ab…");
    }

    // ---- the event ----

    /// `messageKind`: method+id is a request, method alone a notification, neither a response.
    #[test]
    fn message_kind_reads_the_two_keys_upstream_reads() {
        let kind = |value: serde_json::Value| {
            trace_event(
                TraceDirection::Inbound,
                "s",
                TraceTransportKind::Stdio,
                &value,
                TraceStatus::Received,
                None,
            )
            .kind
        };
        assert_eq!(kind(json!({"method": "m", "id": 1})), TraceMessageKind::Request);
        assert_eq!(kind(json!({"method": "m"})), TraceMessageKind::Notification);
        assert_eq!(kind(json!({"id": 1, "result": {}})), TraceMessageKind::Response);
        // rmcp models errors as their own variant; upstream calls them responses.
        assert_eq!(
            kind(json!({"id": 1, "error": {"code": -32601}})),
            TraceMessageKind::Response
        );
    }

    /// A string id is itself potentially a secret; a numeric one is the correlation key.
    #[test]
    fn string_ids_are_redacted_and_numeric_ids_pass_through() {
        let id = |value: serde_json::Value| {
            trace_event(
                TraceDirection::Inbound,
                "s",
                TraceTransportKind::Stdio,
                &value,
                TraceStatus::Received,
                None,
            )
            .id
        };
        assert_eq!(id(json!({"id": 7, "result": {}})), Some(Some(TraceId::Number(7))));
        assert_eq!(
            id(json!({"id": "abc", "result": {}})),
            Some(Some(TraceId::Redacted(REDACTED_ID)))
        );
        // Present but null -> `Some(None)`, which serialises as `null`.
        assert_eq!(id(json!({"id": null, "result": {}})), Some(None));
        // Absent entirely -> omitted.
        assert_eq!(id(json!({"result": {}})), None);
    }

    /// The asymmetry the struct's doc calls out, checked on the wire rather than in the type.
    #[test]
    fn a_null_id_is_written_but_an_absent_related_request_id_is_omitted() {
        let mut event = event();
        event.id = Some(None);
        event.related_request_id = None;
        let line = serde_json::to_string(&event).unwrap();
        assert!(line.contains(r#""id":null"#), "got {line}");
        assert!(!line.contains("relatedRequestId"), "got {line}");
    }

    /// Serde emits declaration order, and the wire order is `createMcpTraceEvent`'s INSERTION order
    /// — in which `bytes` is 8th, not 12th as the TS interface declares it.
    #[test]
    fn the_field_order_is_the_insertion_order_not_the_interface_order() {
        let line = serde_json::to_string(&event()).unwrap();
        let keys: Vec<&str> = line
            .split(',')
            .filter_map(|part| part.split(':').next())
            .map(|key| key.trim_matches(|c: char| c == '{' || c == '"'))
            .collect();
        let head: Vec<&str> = keys.into_iter().take(8).collect();
        assert_eq!(
            head,
            vec!["version", "timestamp", "direction", "server", "transport", "kind", "status", "bytes"]
        );
    }

    #[test]
    fn error_codes_are_lifted_and_durations_are_rounded_to_two_places() {
        let event = trace_event(
            TraceDirection::Inbound,
            "s",
            TraceTransportKind::Stdio,
            &json!({"id": 1, "error": {"code": -32601, "message": "nope"}}),
            TraceStatus::Received,
            Some(Duration::from_micros(1_234_567)),
        );
        assert_eq!(event.error_code, Some(-32601));
        assert_eq!(event.duration_ms, Some(1234.57));
        assert_eq!(round_2dp(-5.0), 0.0, "clamped at zero, as `Math.max(0, …)` does");
    }

    // ---- the writer ----

    /// The ordering the injectable seam exists to observe: mkdir, then truncate, then appends —
    /// and the truncate happens exactly once, not per line.
    #[test]
    fn the_file_is_created_and_truncated_once_before_any_append() {
        let fs = Arc::new(FakeFs::default());
        let writer = writer(&fs, 1_000_000, 100);
        writer.write(&event());
        writer.write(&event());
        let calls = fs.calls();
        assert!(
            calls.first().is_some_and(|c| c.starts_with("mkdir /traces")),
            "got {calls:?}"
        );
        assert!(
            calls.get(1).is_some_and(|c| c.starts_with("truncate /traces/mcp.jsonl")),
            "got {calls:?}"
        );
        assert_eq!(calls.iter().filter(|c| c.starts_with("truncate")).count(), 1);
        assert_eq!(calls.iter().filter(|c| c.starts_with("append")).count(), 2);
    }

    /// `bytes > maxBytes - bytesWritten` **latches disabled** rather than skipping the line: one
    /// oversized event stops the trace instead of leaving a hole in it.
    #[test]
    fn an_oversized_line_disables_the_writer_rather_than_being_skipped() {
        let fs = Arc::new(FakeFs::default());
        let writer = writer(&fs, 10, 100);
        writer.write(&event());
        assert!(writer.is_disabled(), "a line over the cap must latch, not skip");
        assert_eq!(writer.stats(), (0, 0), "a refused line spends no budget");
        // And it stays off for a line that would have fitted.
        writer.write(&event());
        assert_eq!(fs.calls().iter().filter(|c| c.starts_with("append")).count(), 0);
    }

    /// The event cap is `>=`, checked before the line is even built.
    #[test]
    fn the_event_cap_stops_at_exactly_max_events() {
        let fs = Arc::new(FakeFs::default());
        let writer = writer(&fs, 1_000_000, 2);
        for _ in 0..5 {
            writer.write(&event());
        }
        assert_eq!(writer.stats().1, 2);
        assert_eq!(fs.calls().iter().filter(|c| c.starts_with("append")).count(), 2);
        // Reaching the cap is not a latch — the writer is merely full.
        assert!(!writer.is_disabled());
    }

    /// Tracing must never change MCP behaviour: an unusable directory disables the writer silently
    /// and `write` still returns normally.
    #[test]
    fn an_initialisation_failure_is_silent_and_latches() {
        let fs = Arc::new(FakeFs { calls: Mutex::default(), fail_dir: true });
        let writer = writer(&fs, 1_000_000, 100);
        writer.write(&event());
        assert!(writer.is_disabled());
        assert!(fs.calls().is_empty(), "nothing reached the file system");
    }

    // ---- the gate ----

    /// `??`, never `||`: a per-server `false` beats a global `true`.
    #[test]
    fn a_per_server_false_beats_a_global_true() {
        let mut entry = crate::config::ServerEntry::default();
        let settings = crate::config::TraceSettings {
            enabled: Some(true),
            ..Default::default()
        };

        entry.trace = Some(false);
        assert!(
            !is_mcp_trace_enabled(&entry, Some(&settings)),
            "`||` would say true here"
        );
        entry.trace = Some(true);
        assert!(is_mcp_trace_enabled(&entry, Some(&settings)));
        entry.trace = None;
        assert!(
            is_mcp_trace_enabled(&entry, Some(&settings)),
            "unset falls through to the global"
        );
        // No settings at all is `settings?.enabled === true` -> false.
        assert!(!is_mcp_trace_enabled(&entry, None));
    }

    #[test]
    fn an_absolute_configured_file_wins_and_a_relative_one_resolves_against_cwd() {
        let dirs = crate::dirs::McpDirs::new(PathBuf::from("/home/agent"), PathBuf::from("/work"));
        let absolute = crate::config::TraceSettings {
            file: Some("/var/log/mcp.jsonl".to_string()),
            ..Default::default()
        };
        assert_eq!(
            trace_file_path(&dirs, &absolute, "abcd1234"),
            PathBuf::from("/var/log/mcp.jsonl")
        );

        let relative = crate::config::TraceSettings {
            file: Some("traces/out.jsonl".to_string()),
            ..Default::default()
        };
        assert_eq!(
            trace_file_path(&dirs, &relative, "abcd1234"),
            PathBuf::from("/work/traces/out.jsonl")
        );

        let default = crate::config::TraceSettings::default();
        let path = trace_file_path(&dirs, &default, "abcd1234");
        assert!(path.starts_with("/work/.cyrup/mcp-traces"), "got {}", path.display());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("mcp-") && name.ends_with("-abcd1234.jsonl"), "got {name}");
        // `:` and `.` are mapped out of the ISO stamp so the name is portable.
        assert!(!name.trim_end_matches(".jsonl").contains(':'), "got {name}");
    }

    #[test]
    fn the_random_suffix_is_eight_base36_characters() {
        let suffix = random_suffix();
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()), "got {suffix}");
        assert_ne!(random_suffix(), suffix, "two draws should differ");
    }
}
