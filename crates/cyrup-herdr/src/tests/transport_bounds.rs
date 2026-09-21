//! The two framing bounds, and the two protocols they do **not** come from.
//!
//! The JSON socket API bounds a request line at 1 MiB — `MAX_INITIAL_REQUEST_BYTES`
//! (`tmp/herdr/src/api/server.rs:32`). The 32 MiB figure that circulates is
//! `MAX_GRAPHICS_FRAME_SIZE` (`tmp/herdr/src/protocol/wire.rs:29`) on herdr's separate **binary**
//! client-shell protocol. herdr sets no response bound at all, so the 4 MiB one is this client's,
//! taken from pi's `MAX_RPC_BYTES` (`src/runs/shared/herdr-connection.ts:9` @v0.68.0).

use std::time::Duration;

use tokio::io::BufReader;

use super::fake_server::{FakeHerdr, Reply, request_id};
use crate::error::HerdrError;
use crate::probe::ping_for;
use crate::schema::{Method, PingParams, Request};
use crate::transport::{self, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};

/// A request line over 1 MiB is refused **before a byte is written**, naming the bound — rather
/// than herdr closing the connection with `"api request line is too large"`
/// (`tmp/herdr/src/api/server.rs:563-567`) and this client reporting a bare EOF.
///
/// **The boundary is herdr's, to the byte.** `read_initial_request_line_with_limits`
/// (`server.rs:556-568`) pushes each byte and then tests `bytes.len() > max_bytes`, but the
/// `b'\n'` branch above that test breaks out first — so the terminator is never counted and a
/// JSON line of exactly `MAX_REQUEST_BYTES` is **accepted** by a real herdr. A client that
/// counted its own newline would refuse a `pane.send_input` herdr would have executed, and the
/// feature would stop working at exactly the size herdr allows with nothing to say why. So the
/// three cases below are pinned together: `MAX_REQUEST_BYTES - 1` and `MAX_REQUEST_BYTES` are
/// written, `MAX_REQUEST_BYTES + 1` is refused.
#[tokio::test]
async fn a_request_line_over_one_mib_is_refused_before_it_is_written() {
    // One request per connection, here too: the fake reads one line and closes, exactly as
    // `handle_connection_with_stop` does, so each of the three lines below gets its own socket
    // rather than a second write onto a half that herdr has already dropped.
    let fake = FakeHerdr::start(Reply::Close);
    let write = async |line: &str| {
        let mut stream = tokio::net::UnixStream::connect(fake.path())
            .await
            .expect("connect to the fake");
        transport::write_line(&mut stream, "ping", line).await
    };

    let under = "x".repeat(MAX_REQUEST_BYTES - 1);
    let exactly = "x".repeat(MAX_REQUEST_BYTES);
    let over = "x".repeat(MAX_REQUEST_BYTES + 1);

    write(&under)
        .await
        .expect("a line under the bound is written");

    // The boundary itself, which herdr accepts: `bytes.len() > max_bytes` is only reached for a
    // non-newline byte, so 1 MiB of JSON plus its terminator is a request herdr executes.
    write(&exactly)
        .await
        .expect("a line of exactly MAX_REQUEST_BYTES is herdr's own ceiling, not one past it");

    let error = write(&over).await.unwrap_err();
    assert!(
        matches!(
            &error,
            HerdrError::TooLarge {
                method: "ping",
                limit
            } if *limit == MAX_REQUEST_BYTES
        ),
        "expected the 1 MiB request bound, got {error:?}"
    );
    assert_eq!(MAX_REQUEST_BYTES, 1024 * 1024);
}

/// An unbounded read on a socket is an unbounded allocation. herdr bounds what it *reads* and
/// nothing bounds what it *writes*, so this bound is the client's own.
#[tokio::test]
async fn a_response_line_over_four_mib_is_refused() {
    let (client, mut server) = tokio::net::UnixStream::pair().expect("socket pair");
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let chunk = vec![b'x'; 64 * 1024];
        // No newline, ever: the bound must fire while reading, not after a complete line.
        loop {
            if server.write_all(&chunk).await.is_err() {
                return;
            }
        }
    });

    let mut reader = BufReader::new(client);
    let error = transport::read_line(&mut reader, "session.snapshot")
        .await
        .unwrap_err();
    writer.abort();

    assert!(
        matches!(
            &error,
            HerdrError::TooLarge {
                method: "session.snapshot",
                limit
            } if *limit == MAX_RESPONSE_BYTES
        ),
        "expected the 4 MiB response bound, got {error:?}"
    );
    assert_eq!(MAX_RESPONSE_BYTES, 4 * 1024 * 1024);
}

/// A reply just under the response bound still round-trips — the bound must not be so eager that it
/// refuses the one genuinely large answer (`session.snapshot` over a big session).
#[tokio::test]
async fn a_large_but_bounded_response_still_round_trips() {
    let fake = FakeHerdr::start_with(|request| {
        // ~1 MiB of padding inside a well-formed pong.
        let padding = "p".repeat(1024 * 1024);
        Reply::Line(format!(
            r#"{{"id":"{}","result":{{"type":"pong","version":"0.9.1","protocol":22,"padding":"{padding}"}}}}"#,
            request_id(request)
        ))
    });

    let pong = ping_for(fake.path(), Duration::from_secs(10))
        .await
        .expect("a megabyte answer is inside the bound");
    assert_eq!(pong.protocol(), 22);
}

/// herdr closes without writing when the request line is empty or the connection drops
/// (`tmp/herdr/src/api/server.rs:168-175`). That is `Closed`, distinctly — not a timeout, not a
/// malformed line, and certainly not a success.
#[tokio::test]
async fn a_server_that_closes_without_answering_is_closed_not_timeout() {
    let fake = FakeHerdr::start(Reply::Close);

    let error = transport::request(
        fake.path(),
        &Request {
            id: "cyrup:ping:1".to_owned(),
            method: Method::Ping(PingParams {}),
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, HerdrError::Closed { method: "ping" }),
        "expected Closed, got {error:?}"
    );
}

/// A line that is not a response envelope at all is `Malformed`, naming the method — never a
/// silently dropped answer.
#[tokio::test]
async fn a_line_that_is_not_a_response_is_malformed() {
    let fake = FakeHerdr::start(Reply::Line("not json at all".to_owned()));

    let error = ping_for(fake.path(), Duration::from_secs(5))
        .await
        .unwrap_err();

    assert!(
        matches!(&error, HerdrError::Malformed { method: "ping", .. }),
        "expected Malformed, got {error:?}"
    );
}
