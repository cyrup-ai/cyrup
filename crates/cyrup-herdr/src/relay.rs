//! The placed-run relay: the view of a child that runs in a herdr pane on a saved SSH machine,
//! carried back to this machine through [`SshTransport`] — the workspace's one herdr client —
//! with no local shell in between.
//!
//! pi-subagents keeps a live view of its pane-native remote Pi through a bridge socket it forwards
//! over ssh and reconnects when ssh drops (`BridgeChannel`, `reconnectHerdrPiSession`,
//! `boundedHerdrReconnect` — `src/runs/shared/herdr-placed-run.ts` @v0.68.0). The remote side of
//! that bridge is an extension INSIDE the remote Pi. A placed cyrup child's contract is files — its
//! NDJSON event stream, its supervisor channel, its steer inbox and acknowledgements — so the
//! remote side here is a small POSIX script that reads those files where the child wrote them
//! ([`RELAY_SCRIPT`]), and the local side is this module:
//!
//! * **one long-lived ssh per connection** ([`SshTransport::stream`]) carries a framed stream back:
//!   the child's event bytes from a byte offset, every mirrored channel file whose content changed
//!   (and the removal of one that disappeared), the child's stderr tail, and its exit code;
//! * **bounded ssh commands** ([`SshTransport::run`]) carry the other direction: every file the
//!   parent drops into a mirrored local directory (a steer request, a supervisor reply) is uploaded
//!   atomically into the run's remote directory and then removed locally — delivered;
//! * **bounded reconnect**: when the stream's ssh ends without the exit frame, a fresh one resumes
//!   from the byte offset already delivered, so no event is lost or repeated — upstream's budget,
//!   three attempts inside fifteen seconds per loss ([`RECONNECT_ATTEMPTS`],
//!   [`RECONNECT_DEADLINE`]). Past it the child's state is unknown ([`RelayEnd::Unknown`]).
//!
//! ssh never owns the child: every ssh here only reads what the pane's child already wrote, so
//! losing one loses the view, never the work (`herdr-machine.ts:10-17`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::remote::{SshTransport, remote_shell_command};

/// The relay frame protocol version the remote script announces in its hello frame.
pub const RELAY_PROTOCOL: u32 = 1;
/// `boundedHerdrReconnect`'s attempt budget (`herdr-placed-run.ts:35-45`, `attempts: 3`).
pub const RECONNECT_ATTEMPTS: u32 = 3;
/// `boundedHerdrReconnect`'s deadline (`deadlineMs: 15_000`).
pub const RECONNECT_DEADLINE: Duration = Duration::from_secs(15);
/// How often the parent's outbound channel directories are checked — the child-side steer inbox
/// poll period (`subagent-prompt-runtime.ts:432`), so a relayed request adds at most one period.
pub const UPLOAD_POLL: Duration = Duration::from_millis(250);
/// One event frame's upper bound; the child protocol's own per-line cap is 16 MiB.
pub const MAX_EVENT_FRAME: u64 = 16 * 1024 * 1024;
/// One mirrored channel file's upper bound (supervisor messages are capped at 64 KiB upstream).
pub const MAX_FILE_FRAME: u64 = 1024 * 1024;
/// One frame header's upper bound.
const MAX_HEADER: u64 = 1024;
/// One uploaded channel file's upper bound — pi's bridge refuses request text over 256 KiB.
pub const MAX_UPLOAD_BYTES: u64 = 1024 * 1024;
/// The deadline of one upload command.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// The remote half: stream the run directory's event file from byte offset `$3`, mirror the named
/// channel files, report the child's exit — or report the child LOST when its `run.sh` died
/// without recording an exit (its pane was closed or killed).
///
/// Positional: `$1` runtime dir, `$2` run id, `$3` event offset, then one word per mirror —
/// `d:<key>:<relative dir>` (every `*.json` in it) or `f:<key>:<relative file>`.
///
/// Frames (one header line each, payload bytes after it where a length is given):
/// `H <protocol>` hello · `E <n>` event bytes · `F <key> <name> <n>` a mirrored file's content
/// (`-` as the name for a single-file mirror) · `D <key> <name>` a mirrored file is gone ·
/// `S <n>` the child's stderr tail · `X <code>` the child exited · `L <sentence>` the child was
/// lost. The script exits 0 after `X`/`L`; any other end is a lost transport.
pub const RELAY_SCRIPT: &str = r#"rt=$1; id=$2; off=$3; shift 3
case "${rt##*/}" in cyrup-subagents-herdr-*) ;; *) echo "not a placed-run runtime directory" >&2; exit 64;; esac
if [ ! -d "$rt" ] || [ -L "$rt" ]; then echo "The placed run's runtime directory is gone." >&2; exit 66; fi
if [ "$(cat "$rt/run-id" 2>/dev/null)" != "$id" ]; then echo "The placed run's identity changed." >&2; exit 65; fi
printf 'H 1\n'
n=0
while [ ! -e "$rt/started" ]; do
  n=$((n+1))
  if [ "$n" -gt 300 ]; then printf 'L The placed cyrup child did not start in its Herdr pane within 60 s.\n'; exit 0; fi
  sleep 0.2 2>/dev/null || sleep 1
done
f="$rt/events.ndjson"; snap="$rt/.relay-snap.$$"; old="$rt/.relay-old.$$"; new="$rt/.relay-new.$$"
: > "$old"
trap 'rm -f "$snap" "$old" "$new"' EXIT
emit() {
  cat "$3" > "$snap" 2>/dev/null || return 0
  sig=$(cksum < "$snap" | tr ' ' '_')
  printf '%s/%s %s\n' "$1" "$2" "$sig" >> "$new"
  prev=$(awk -v k="$1/$2" '$1==k{print $2}' "$old")
  [ "$prev" = "$sig" ] && return 0
  s=$(wc -c < "$snap"); s=$((s+0))
  printf 'F %s %s %s\n' "$1" "$2" "$s"; cat "$snap"
}
mirror() {
  : > "$new"
  for spec in "$@"; do
    kind=${spec%%:*}; rest=${spec#*:}; key=${rest%%:*}; rel=${rest#*:}
    if [ "$kind" = d ]; then
      for p in "$rt/$rel"/*.json; do
        if [ ! -f "$p" ] || [ -L "$p" ]; then continue; fi
        name=${p##*/}
        case "$name" in .*|*[!A-Za-z0-9._-]*) continue;; esac
        emit "$key" "$name" "$p"
      done
    elif [ -f "$rt/$rel" ] && [ ! -L "$rt/$rel" ]; then
      emit "$key" - "$rt/$rel"
    fi
  done
  awk -v n="$new" 'BEGIN{while((getline l < n)>0){split(l,a," ");k[a[1]]=1}} !($1 in k){print $1}' "$old" | while IFS= read -r gone; do printf 'D %s %s\n' "${gone%%/*}" "${gone#*/}"; done
  mv -f "$new" "$old"
}
events() {
  if [ -f "$f" ]; then size=$(wc -c < "$f"); size=$((size+0)); else size=0; fi
  if [ "$size" -gt "$off" ]; then len=$((size-off)); printf 'E %s\n' "$len"; tail -c +"$((off+1))" "$f" | head -c "$len"; off=$size; fi
}
while :; do
  fin=0
  if [ -e "$rt/exit" ]; then fin=1
  elif [ -s "$rt/pid" ] && ! kill -0 "$(cat "$rt/pid")" 2>/dev/null && [ ! -e "$rt/exit" ]; then fin=2
  fi
  events
  mirror "$@"
  if [ "$fin" != 0 ]; then
    if [ -s "$rt/stderr" ]; then tail -c 8192 "$rt/stderr" > "$snap"; s=$(wc -c < "$snap"); printf 'S %s\n' "$((s+0))"; cat "$snap"; fi
    if [ "$fin" = 1 ]; then
      code=$(cat "$rt/exit" 2>/dev/null); case "$code" in ''|*[!0-9]*) code=1;; esac
      printf 'X %s\n' "$code"
    else
      printf 'L The placed cyrup child exited without recording its exit status; its Herdr pane was closed or killed.\n'
    fi
    exit 0
  fi
  sleep 0.2 2>/dev/null || sleep 1
done"#;

/// Upload one channel file atomically: `$1` runtime dir, `$2` relative dir, `$3` file name; the
/// content arrives on stdin and is renamed into place only once complete, so a child never reads a
/// partial request.
const UPLOAD_SCRIPT: &str = "umask 077; d=\"$1/$2\"; mkdir -p \"$d\" && cat > \"$d/.$3.part\" && mv -f \"$d/.$3.part\" \"$d/$3\"";

/// Whether a mirror is a directory of `*.json` files or one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorKind {
    /// Every `*.json` file in the directory.
    Dir,
    /// One file.
    File,
}

/// A remote channel path mirrored onto a local one, remote → local.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownMirror {
    /// The frame key (`[A-Za-z0-9_-]+`).
    pub key: String,
    /// The path relative to the run's remote directory.
    pub remote: String,
    /// The local directory (for [`MirrorKind::Dir`]) or file ([`MirrorKind::File`]).
    pub local: PathBuf,
    /// Directory or file.
    pub kind: MirrorKind,
}

/// A local directory whose `*.json` files are DELIVERED into the run's remote directory,
/// local → remote: uploaded, then removed locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpMirror {
    /// The local directory the parent writes into.
    pub local: PathBuf,
    /// The directory relative to the run's remote directory.
    pub remote: String,
}

/// How a relay ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayEnd {
    /// The child exited with this code; every byte it wrote has been delivered.
    Exited(i32),
    /// The child ended without recording an exit (its pane was closed or killed), or never started.
    Lost(String),
    /// The transport was lost past the reconnect budget, or refused the run's identity: the
    /// child's state is unknown.
    Unknown(String),
}

/// One placed run's relay.
#[derive(Clone, Debug)]
pub struct RunRelay {
    transport: SshTransport,
    target: String,
    runtime_dir: String,
    run_id: String,
    down: Vec<DownMirror>,
    up: Vec<UpMirror>,
}

/// Everything one stream connection learned before it ended.
enum StreamEnd {
    /// `X`/`L` arrived.
    Final(RelayEnd),
    /// The stream ended without a final frame. `hello` says whether the remote side started.
    Lost { hello: bool, reason: String },
    /// The remote side refused the run (wrong directory or identity) — not worth retrying.
    Refused(String),
}

/// A frame header line.
#[derive(Debug, PartialEq, Eq)]
enum Header {
    Hello(u32),
    Events(u64),
    File { key: String, name: String, len: u64 },
    Gone { key: String, name: String },
    Stderr(u64),
    Exit(i32),
    Lost(String),
}

fn parse_header(line: &str) -> Option<Header> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
    let mut words = rest.split(' ');
    match tag {
        "H" => rest.parse().ok().map(Header::Hello),
        "E" => rest.parse().ok().map(Header::Events),
        "S" => rest.parse().ok().map(Header::Stderr),
        "X" => rest.parse().ok().map(Header::Exit),
        "L" => Some(Header::Lost(rest.to_string())),
        "F" => {
            let key = words.next()?.to_string();
            let name = words.next()?.to_string();
            let len = words.next()?.parse().ok()?;
            words
                .next()
                .is_none()
                .then_some(Header::File { key, name, len })
        }
        "D" => {
            let key = words.next()?.to_string();
            let name = words.next()?.to_string();
            words.next().is_none().then_some(Header::Gone { key, name })
        }
        _ => None,
    }
}

/// A file name the relay will write locally or upload: `[A-Za-z0-9._-]+`, not hidden.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Write `bytes` to `path` through a sibling temp file and a rename, mode 0600.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("a mirrored path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{name}.relay-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

impl RunRelay {
    /// A relay for the run in `runtime_dir` on `target`, identified by `run_id` (the remote script
    /// refuses a directory whose `run-id` file says otherwise).
    #[must_use]
    pub fn new(
        transport: SshTransport,
        target: impl Into<String>,
        runtime_dir: impl Into<String>,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            transport,
            target: target.into(),
            runtime_dir: runtime_dir.into(),
            run_id: run_id.into(),
            down: Vec::new(),
            up: Vec::new(),
        }
    }

    /// Mirror a remote channel path onto a local one.
    #[must_use]
    pub fn with_down(mut self, mirror: DownMirror) -> Self {
        self.down.push(mirror);
        self
    }

    /// Deliver a local directory's files into the run's remote directory.
    #[must_use]
    pub fn with_up(mut self, mirror: UpMirror) -> Self {
        self.up.push(mirror);
        self
    }

    /// The remote command for one connection resuming at `offset`.
    fn command(&self, offset: u64) -> String {
        let offset = offset.to_string();
        let mut args: Vec<String> = vec![self.runtime_dir.clone(), self.run_id.clone(), offset];
        for mirror in &self.down {
            let kind = match mirror.kind {
                MirrorKind::Dir => "d",
                MirrorKind::File => "f",
            };
            args.push(format!("{kind}:{}:{}", mirror.key, mirror.remote));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        remote_shell_command(RELAY_SCRIPT, &refs)
    }

    /// Run the relay to its end, writing the child's event bytes to `events` and its stderr tail
    /// (and any relay diagnostic) to `stderr`.
    pub async fn run<E, S>(self, mut events: E, mut stderr: S) -> RelayEnd
    where
        E: AsyncWrite + Unpin,
        S: AsyncWrite + Unpin,
    {
        let mut state = DeliveryState::default();
        let end = {
            let uploads = self.upload_loop();
            tokio::pin!(uploads);
            let stream = self.stream_loop(&mut state, &mut events, &mut stderr);
            tokio::pin!(stream);
            tokio::select! {
                end = &mut stream => end,
                () = &mut uploads => std::future::pending::<RelayEnd>().await,
            }
        };
        let _ = events.shutdown().await;
        if let RelayEnd::Unknown(reason) | RelayEnd::Lost(reason) = &end {
            let _ = stderr.write_all(format!("{reason}\n").as_bytes()).await;
        }
        let _ = stderr.shutdown().await;
        end
    }

    /// The remote → local half, reconnecting within the budget.
    async fn stream_loop<E, S>(
        &self,
        state: &mut DeliveryState,
        events: &mut E,
        stderr: &mut S,
    ) -> RelayEnd
    where
        E: AsyncWrite + Unpin,
        S: AsyncWrite + Unpin,
    {
        // `boundedHerdrReconnect`: per LOSS, up to three attempts inside fifteen seconds. A
        // connection that reached the remote side (its hello frame) ends the episode.
        let mut episode: Option<(tokio::time::Instant, u32)> = None;
        loop {
            match self.connect_once(state, events, stderr).await {
                StreamEnd::Final(end) => return end,
                StreamEnd::Refused(reason) => {
                    return RelayEnd::Unknown(format!(
                        "Pane-native reconnect remains unknown: {reason}"
                    ));
                }
                StreamEnd::Lost { hello, reason } => {
                    if hello {
                        episode = None;
                    }
                    let now = tokio::time::Instant::now();
                    let (deadline, used) = episode.get_or_insert((now + RECONNECT_DEADLINE, 0));
                    if *used >= RECONNECT_ATTEMPTS || now >= *deadline {
                        return RelayEnd::Unknown(format!(
                            "Pane-native reconnect remains unknown: {reason}; the remote child's state is unknown and its pane is retained for inspection."
                        ));
                    }
                    *used += 1;
                    let backoff = Duration::from_millis(100 * u64::from(*used))
                        .min(deadline.saturating_duration_since(now));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// One ssh connection: read frames until a final one or the end of the stream.
    async fn connect_once<E, S>(
        &self,
        state: &mut DeliveryState,
        events: &mut E,
        stderr: &mut S,
    ) -> StreamEnd
    where
        E: AsyncWrite + Unpin,
        S: AsyncWrite + Unpin,
    {
        let mut child = match self
            .transport
            .stream(&self.target, &self.command(state.offset))
        {
            Ok(child) => child,
            Err(error) => {
                return StreamEnd::Lost {
                    hello: false,
                    reason: error.to_string(),
                };
            }
        };
        let (Some(stdout), Some(ssh_stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return StreamEnd::Lost {
                hello: false,
                reason: "the relay's ssh pipes were not available".to_string(),
            };
        };
        // ssh's own diagnostics, kept for the loss sentence (bounded).
        let diagnostics = tokio::spawn(async move {
            let mut out = Vec::new();
            let _ = ssh_stderr.take(64 * 1024).read_to_end(&mut out).await;
            String::from_utf8_lossy(&out).trim().to_string()
        });
        let mut reader = BufReader::new(stdout);
        let mut hello = false;
        let outcome = self
            .read_frames(&mut reader, state, events, stderr, &mut hello)
            .await;
        drop(reader);
        let status = child.wait().await.ok().and_then(|status| status.code());
        let diagnostic = diagnostics.await.unwrap_or_default();
        match outcome {
            Ok(Some(end)) => StreamEnd::Final(end),
            Ok(None) | Err(_) => {
                let reason = match (status, diagnostic.is_empty()) {
                    (Some(code), false) => {
                        format!("the ssh relay exited with code {code}: {diagnostic}")
                    }
                    (Some(code), true) => format!("the ssh relay exited with code {code}"),
                    (None, false) => format!("the ssh relay was lost: {diagnostic}"),
                    (None, true) => "the ssh relay was lost".to_string(),
                };
                if !hello && matches!(status, Some(64..=66)) {
                    StreamEnd::Refused(if diagnostic.is_empty() {
                        reason
                    } else {
                        diagnostic
                    })
                } else {
                    StreamEnd::Lost { hello, reason }
                }
            }
        }
    }

    /// Read frames off one connection. `Ok(Some(_))` is a final frame, `Ok(None)` the end of the
    /// stream without one; `Err` a malformed or over-long frame (treated as a lost transport).
    async fn read_frames<R, E, S>(
        &self,
        reader: &mut R,
        state: &mut DeliveryState,
        events: &mut E,
        stderr: &mut S,
        hello: &mut bool,
    ) -> std::io::Result<Option<RelayEnd>>
    where
        R: tokio::io::AsyncBufRead + Unpin,
        E: AsyncWrite + Unpin,
        S: AsyncWrite + Unpin,
    {
        loop {
            let mut raw = Vec::new();
            let read = (&mut *reader)
                .take(MAX_HEADER)
                .read_until(b'\n', &mut raw)
                .await?;
            if read == 0 {
                return Ok(None);
            }
            if raw.last() != Some(&b'\n') {
                return Err(std::io::Error::other("a relay frame header was truncated"));
            }
            let line = String::from_utf8_lossy(&raw);
            let Some(header) = parse_header(&line) else {
                return Err(std::io::Error::other(format!(
                    "the relay sent a malformed frame: {}",
                    line.trim()
                )));
            };
            if !*hello && !matches!(header, Header::Hello(_)) {
                return Err(std::io::Error::other("the relay did not announce itself"));
            }
            match header {
                Header::Hello(protocol) => {
                    if protocol != RELAY_PROTOCOL {
                        return Err(std::io::Error::other("relay protocol mismatch"));
                    }
                    *hello = true;
                }
                Header::Events(len) => {
                    if len > MAX_EVENT_FRAME {
                        return Err(std::io::Error::other("an event frame exceeded its bound"));
                    }
                    let copied =
                        tokio::io::copy(&mut (&mut *reader).take(len), &mut *events).await?;
                    events.flush().await?;
                    state.offset += copied;
                    if copied != len {
                        return Ok(None);
                    }
                }
                Header::File { key, name, len } => {
                    if len > MAX_FILE_FRAME {
                        return Err(std::io::Error::other("a mirrored file exceeded its bound"));
                    }
                    let mut bytes = Vec::new();
                    (&mut *reader).take(len).read_to_end(&mut bytes).await?;
                    if bytes.len() as u64 != len {
                        return Ok(None);
                    }
                    self.deliver_down(state, &key, &name, bytes);
                }
                Header::Gone { key, name } => self.remove_down(state, &key, &name),
                Header::Stderr(len) => {
                    let mut bytes = Vec::new();
                    (&mut *reader)
                        .take(len.min(MAX_FILE_FRAME))
                        .read_to_end(&mut bytes)
                        .await?;
                    stderr.write_all(&bytes).await?;
                }
                Header::Exit(code) => return Ok(Some(RelayEnd::Exited(code))),
                Header::Lost(reason) => return Ok(Some(RelayEnd::Lost(reason))),
            }
        }
    }

    fn mirror_target(&self, key: &str, name: &str) -> Option<PathBuf> {
        let mirror = self.down.iter().find(|mirror| mirror.key == key)?;
        match mirror.kind {
            MirrorKind::File => (name == "-").then(|| mirror.local.clone()),
            MirrorKind::Dir => safe_name(name).then(|| mirror.local.join(name)),
        }
    }

    /// Deliver one mirrored file, once per distinct content — a reconnected stream re-announces
    /// every file it finds, and the parent may already have consumed (removed) the copy it got.
    fn deliver_down(&self, state: &mut DeliveryState, key: &str, name: &str, bytes: Vec<u8>) {
        let Some(path) = self.mirror_target(key, name) else {
            return;
        };
        let id = (key.to_string(), name.to_string());
        if state.delivered.get(&id) == Some(&bytes) {
            return;
        }
        if write_atomic(&path, &bytes).is_ok() {
            state.delivered.insert(id, bytes);
        }
    }

    /// A mirrored file is gone on the machine: remove the local copy — but only when it is still
    /// exactly what was delivered, so nothing the parent wrote itself is ever removed.
    fn remove_down(&self, state: &mut DeliveryState, key: &str, name: &str) {
        let Some(path) = self.mirror_target(key, name) else {
            return;
        };
        let id = (key.to_string(), name.to_string());
        if let Some(delivered) = state.delivered.remove(&id)
            && std::fs::read(&path).is_ok_and(|current| current == delivered)
        {
            let _ = std::fs::remove_file(&path);
        }
    }

    /// The local → remote half: forever, every [`UPLOAD_POLL`], deliver each new file.
    async fn upload_loop(&self) {
        if self.up.is_empty() {
            return std::future::pending().await;
        }
        let mut tick = tokio::time::interval(UPLOAD_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for mirror in &self.up {
                self.upload_dir(mirror).await;
            }
        }
    }

    /// Deliver every `*.json` file in one local directory, oldest name first. A failed upload
    /// leaves the file for the next tick.
    pub async fn upload_dir(&self, mirror: &UpMirror) {
        let Ok(entries) = std::fs::read_dir(&mirror.local) else {
            return;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| safe_name(name) && name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let path = mirror.local.join(&name);
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes.len() as u64 > MAX_UPLOAD_BYTES {
                continue;
            }
            let sent = self
                .transport
                .run(
                    &self.target,
                    &remote_shell_command(
                        UPLOAD_SCRIPT,
                        &[&self.runtime_dir, &mirror.remote, &name],
                    ),
                    UPLOAD_TIMEOUT,
                    4096,
                    Some(&bytes),
                )
                .await;
            if sent.error.is_none() && sent.status == Some(0) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// What the relay has delivered so far: the event offset and each mirrored file's content.
#[derive(Debug, Default)]
struct DeliveryState {
    offset: u64,
    delivered: BTreeMap<(String, String), Vec<u8>>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn headers_parse_and_refuse_garbage() {
        assert_eq!(parse_header("H 1\n"), Some(Header::Hello(1)));
        assert_eq!(parse_header("E 42\n"), Some(Header::Events(42)));
        assert_eq!(
            parse_header("F sup-req a.json 7\n"),
            Some(Header::File {
                key: "sup-req".to_string(),
                name: "a.json".to_string(),
                len: 7
            })
        );
        assert_eq!(
            parse_header("D sup-req a.json\n"),
            Some(Header::Gone {
                key: "sup-req".to_string(),
                name: "a.json".to_string()
            })
        );
        assert_eq!(parse_header("X 3\n"), Some(Header::Exit(3)));
        assert_eq!(
            parse_header("L gone away\n"),
            Some(Header::Lost("gone away".to_string()))
        );
        assert_eq!(parse_header("F a b\n"), None);
        assert_eq!(parse_header("E x\n"), None);
        assert_eq!(parse_header("Q\n"), None);
        assert!(!safe_name("../x.json"));
        assert!(!safe_name(".hidden.json"));
        assert!(safe_name("0-req_1.json"));
    }
}
