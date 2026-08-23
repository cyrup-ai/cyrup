//! THROWAWAY perf probe (TUI-092 round 2). Not part of the suite — delete after measuring.
//! Reads `App::scrollback_lines`, which exists only under `scrollback-accumulator` (TUI-092 F1):
//! run with `cargo test -p cyrup-tui --features scrollback-accumulator --test zzz_scratch_perf_probe`.
#![cfg(feature = "scrollback-accumulator")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::print_stdout
)]

use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_tui::{reanchor_inline_region, App, RebuildBackend, UiTheme};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use serde_json::json;

const W: u16 = 100;
const H: u16 = 40;

static BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        BYTES.fetch_add(buf.len(), Ordering::Relaxed);
        // Do NOT retain the bytes (we only count them) so the probe's own memory does not
        // masquerade as the app's.
        let _ = &self.0;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CaptureBackend {
    inner: CrosstermBackend<SharedWriter>,
    buf: Arc<Mutex<Vec<u8>>>,
    cursor_y: u16,
}

impl CaptureBackend {
    fn new() -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        CaptureBackend {
            inner: CrosstermBackend::new(SharedWriter(buf.clone())),
            buf,
            cursor_y: H.saturating_sub(1),
        }
    }
}

impl Backend for CaptureBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }
    // Required since TUI-092 F5 turned on ratatui's `scrolling-regions` feature (`425ef9f`), which
    // added both methods to `Backend`. Delegated to the inner crossterm backend exactly as the
    // production `InlineBackend` does (`src/app/backend.rs:193`, `:198`) — this probe captures the
    // real byte stream, so it must emit the same escapes the real backend would.
    //
    // Gated exactly as that production impl (and the two in-src capture backends,
    // `src/tests/inline_stacking.rs:127` and `src/tests/resize_viewport_failure.rs:113`) are:
    // `Backend` declares these two ONLY under `ratatui/scrolling-regions`, and this file self-gates
    // on `scrollback-accumulator`, NOT on `scrolling-regions`. So an ungated impl is `E0407` x2 in a
    // `--no-default-features --features scrollback-accumulator` build — the one combination where
    // the file compiles while the trait methods do not exist.
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, amount)
    }
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, amount)
    }
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.cursor_y = self.cursor_y.saturating_add(n).min(H.saturating_sub(1));
        self.inner.append_lines(n)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(Position { x: 0, y: self.cursor_y })
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        self.cursor_y = p.y;
        self.inner.set_cursor_position(p)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        Ok(Size { width: W, height: H })
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: Size { width: W, height: H },
            pixels: Size { width: 0, height: 0 },
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

impl RebuildBackend for CaptureBackend {
    fn rebuild(&self) -> Self {
        CaptureBackend {
            inner: CrosstermBackend::new(SharedWriter(self.buf.clone())),
            buf: self.buf.clone(),
            cursor_y: self.cursor_y,
        }
    }
    fn reanchor_inline(&mut self, term_height: u16, old_height: u16, new_height: u16) {
        self.cursor_y = reanchor_inline_region(&mut self.inner, term_height, old_height, new_height);
    }
}

// ------------------------------------------------------------------------ realistic content ----

const REPLY: &str = "\
Here is what I found in the parser module.

The tokenizer walks the input byte-by-byte and keeps a `pos` cursor, which means the \
multi-byte UTF-8 path is handled by the `advance_grapheme` helper rather than the main loop. \
That is why the column numbers reported in diagnostics drift on non-ASCII input.

## Proposed fix

1. Track the byte offset and the display column separately.
2. Feed the display column from `unicode_width`, not from the byte delta.
3. Add a regression covering a line that mixes CJK and combining marks.

```rust
fn advance(&mut self, ch: char) {
    self.byte += ch.len_utf8();
    self.col += UnicodeWidthChar::width(ch).unwrap_or(0);
}
```

The change is local to `lexer.rs` and does not touch the public API. I ran the existing suite \
and everything still passes; the new test fails before the patch and passes after it.

Let me know if you want me to apply it.";

fn reply_chunks() -> Vec<String> {
    // Stream in ~40-char chunks, like a provider's token deltas.
    let mut out = Vec::new();
    let bytes: Vec<char> = REPLY.chars().collect();
    for c in bytes.chunks(40) {
        out.push(c.iter().collect::<String>());
    }
    out
}

fn rss_kb() -> u64 {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid]).output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn drive_turn(app: &mut App<CaptureBackend>, i: usize) {
    app.transcript_mut().push_user(format!(
        "turn {i}: the column numbers in the parser diagnostics look wrong on unicode input, can \
         you look into it?"
    ));
    app.state_mut().status.streaming = true;
    app.draw().unwrap();

    // one tool call
    app.transcript_mut().push_tool_start(
        "read",
        json!({"filePath": "/Users/dev/project/crates/parser/src/lexer.rs"}),
    );
    app.draw().unwrap();
    app.transcript_mut().push_tool_end(
        "read",
        false,
        Some(json!({"output": "fn advance(&mut self, ch: char) {\n    self.byte += ch.len_utf8();\n}\n"})),
    );
    app.transcript_mut().commit_tools();
    app.draw().unwrap();

    // streamed assistant reply
    for chunk in reply_chunks() {
        app.transcript_mut().push_assistant_delta(&chunk);
        app.draw().unwrap();
    }
    app.transcript_mut().commit_assistant(None);
    app.state_mut().status.streaming = false;
    app.draw().unwrap();
}

struct Sample {
    idle_us: u128,
    idle_bytes: usize,
    stream_us: u128,
    stream_bytes: usize,
    scrollback: usize,
    rss_kb: u64,
}

fn measure(turns: usize) -> Sample {
    let mut app = App::new(CaptureBackend::new(), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.draw().unwrap();

    for i in 0..turns {
        drive_turn(&mut app, i);
    }

    // ---- IDLE steady state: nothing changed, redraw repeatedly.
    for _ in 0..5 {
        app.draw().unwrap();
    }
    BYTES.store(0, Ordering::Relaxed);
    let n = 40;
    let t0 = Instant::now();
    for _ in 0..n {
        app.draw().unwrap();
    }
    let idle = t0.elapsed();
    let idle_bytes = BYTES.load(Ordering::Relaxed) / n as usize;

    // ---- STREAMING steady state: an in-flight turn of typical size + the spinner running,
    // i.e. exactly the frames the 80ms ticker drives.
    app.state_mut().status.streaming = true;
    app.state_mut().indicator.working();
    app.transcript_mut().push_assistant_delta(&REPLY[..600]);
    for _ in 0..5 {
        app.draw().unwrap();
    }
    BYTES.store(0, Ordering::Relaxed);
    let t1 = Instant::now();
    for _ in 0..n {
        app.draw().unwrap();
    }
    let stream = t1.elapsed();
    let stream_bytes = BYTES.load(Ordering::Relaxed) / n as usize;

    Sample {
        idle_us: idle.as_micros() / n as u128,
        idle_bytes,
        stream_us: stream.as_micros() / n as u128,
        stream_bytes,
        scrollback: app.scrollback_lines().len(),
        rss_kb: rss_kb(),
    }
}

#[test]
fn probe() {
    // warm up syntect/theme statics so turn 1 is not paying for a one-time load
    let _ = measure(1);

    println!(
        "\n turns |  idle draw |  idle bytes | stream draw | stream bytes | scrollback lines | RSS KB"
    );
    println!(
        "-------+------------+-------------+-------------+--------------+------------------+--------"
    );
    for turns in [1usize, 5, 10, 25, 50, 100] {
        let s = measure(turns);
        println!(
            " {:>5} | {:>7} us | {:>11} | {:>8} us | {:>12} | {:>16} | {:>6}",
            turns, s.idle_us, s.idle_bytes, s.stream_us, s.stream_bytes, s.scrollback, s.rss_kb
        );
    }
    let _ = Duration::from_secs(0);
}
