//! One connection's read and write tasks — pi's per-socket callbacks
//! (`broker.ts:196-232`, `framing.ts:29-51`).
//!
//! [`writer_task`] drains queued frames; [`reader_task`] reassembles them, spends a rate-limit
//! token, and dispatches each to [`super::state::BrokerState::handle_frame`] — the switch itself
//! lives in `super::dispatch` — while honoring the 1 s registration timeout. [`spawn_connection`]
//! is the pair's constructor, called from the accept loop in `super::lifecycle`.
//!
//! Split out of `broker/mod.rs` to separate "what one connection does" from "what the process does".

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::transport::framing::FrameReader;
use crate::transport::protocol::{BrokerMessage, now_ms};

use super::frame::{FrameOutcome, send_msg};
use super::lifecycle::schedule_shutdown_check;
use super::limits::{READ_BUF, REGISTRATION_TIMEOUT_MS};
use super::ratelimit::TokenBucket;
use super::routing::SessionKey;
use super::state::{BrokerState, lock};

/// The per-connection writer task: drain queued frames to the socket, then half-close on EOF.
async fn writer_task(
    mut write_half: crate::transport::stream::BrokerWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    while let Some(frame) = rx.recv().await {
        if write_half.write_all(&frame).await.is_err() {
            break;
        }
    }
    let _ = write_half.shutdown().await;
}

/// The outcome of [`process_frame_payload`]: whether the connection should keep reading, and
/// whether the registration timeout must be re-armed (`was_registered && session_key.is_none()` on a
/// frame `handle_frame` flagged `rearmed_registration` for — an `unregister`, `broker.ts:223-230`).
struct PayloadOutcome {
    keep_going: bool,
    rearm_registration: bool,
}

/// Process one fully-reassembled frame payload: rate-limit, JSON-decode, dispatch to
/// [`BrokerState::handle_frame`], and apply its result — pi's per-message `onMessage` callback
/// (`framing.ts:29-47`, `broker.ts:217-230`). `keep_going = false` means tear the connection down,
/// mirroring `onError`'s `socket.destroy(error)` / a fatal [`FrameOutcome`].
fn process_frame_payload(
    payload: &[u8],
    conn_id: u64,
    self_tx: &UnboundedSender<Vec<u8>>,
    state: &Arc<Mutex<BrokerState>>,
    bucket: &mut TokenBucket,
    session_key: &mut Option<SessionKey>,
) -> PayloadOutcome {
    // Rate limit BEFORE handling (broker.ts:218-222).
    if !bucket.consume(now_ms()) {
        send_msg(
            self_tx,
            &BrokerMessage::Error {
                error: "Intercom broker rate limit exceeded".to_string(),
            },
        );
        return PayloadOutcome {
            keep_going: false,
            rearm_registration: false,
        };
    }
    // JS-lenient: an overflowing numeric literal must not kill the whole frame — see
    // `framing::from_frame_slice`.
    let value: serde_json::Value = match crate::transport::framing::from_frame_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            // `reportMessage`'s `JSON.parse` catch (`framing.ts:29-37`): a descriptive diagnostic,
            // then destroy the connection (`onError` -> `socket.destroy(error)`, `broker.ts:231-233`).
            tracing::warn!(
                error = %crate::transport::framing::FrameError::Parse { message: e.to_string() },
                "intercom broker: dropping connection"
            );
            return PayloadOutcome {
                keep_going: false,
                rearm_registration: false,
            };
        }
    };
    let was_registered = session_key.is_some();
    let now = now_ms();
    let result = {
        let mut g = lock(state);
        g.handle_frame(conn_id, self_tx, &value, session_key, now)
    };
    if result.schedule_shutdown {
        schedule_shutdown_check(state);
    }
    let rearm = result.rearmed_registration && was_registered && session_key.is_none();
    PayloadOutcome {
        keep_going: matches!(result.outcome, FrameOutcome::Continue),
        rearm_registration: rearm,
    }
}

/// The per-connection reader task: read chunks, reassemble frames, rate-limit, and dispatch each to
/// [`BrokerState::handle_frame`], honoring the 1 s registration timeout.
async fn reader_task(
    conn_id: u64,
    mut read_half: crate::transport::stream::BrokerReadHalf,
    self_tx: UnboundedSender<Vec<u8>>,
    close: Arc<Notify>,
    state: Arc<Mutex<BrokerState>>,
) {
    // `let sessionKey: string | null = null;` (`v0.13.0 broker/broker.ts:268`).
    let mut session_key: Option<SessionKey> = None;
    let mut bucket = TokenBucket::new(now_ms());
    let mut reader = FrameReader::new();
    let mut buf = vec![0u8; READ_BUF];

    let reg_deadline = tokio::time::sleep(Duration::from_millis(REGISTRATION_TIMEOUT_MS));
    tokio::pin!(reg_deadline);

    'outer: loop {
        tokio::select! {
            biased;
            () = close.notified() => break,
            () = &mut reg_deadline, if session_key.is_none() => {
                // No register within the timeout → destroy (broker.ts:196-201).
                break;
            }
            read = read_half.read(&mut buf) => {
                let n = match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let chunk = buf.get(..n).unwrap_or(&[]);
                let frames = match reader.push(chunk) {
                    Ok(frames) => frames,
                    Err(e) => {
                        // pi's reader delivers every frame reassembled earlier in this SAME chunk to
                        // `onMessage` synchronously, in order, and only afterward discovers/reports the
                        // oversize length (`framing.ts:52-84`) — dispatch `e.frames` before tearing the
                        // connection down, rather than discarding them.
                        for payload in &e.frames {
                            let outcome = process_frame_payload(payload, conn_id, &self_tx, &state, &mut bucket, &mut session_key);
                            if outcome.rearm_registration {
                                reg_deadline
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + Duration::from_millis(REGISTRATION_TIMEOUT_MS));
                            }
                            if !outcome.keep_going {
                                break;
                            }
                        }
                        tracing::warn!(error = %e.error, "intercom broker: dropping connection");
                        break; // oversize → drop the connection (framing.ts:63-66)
                    }
                };
                for payload in &frames {
                    let outcome = process_frame_payload(payload, conn_id, &self_tx, &state, &mut bucket, &mut session_key);
                    if outcome.rearm_registration {
                        reg_deadline
                            .as_mut()
                            .reset(tokio::time::Instant::now() + Duration::from_millis(REGISTRATION_TIMEOUT_MS));
                    }
                    if !outcome.keep_going {
                        break 'outer;
                    }
                }
            }
        }
    }

    // Teardown (socket 'close', broker.ts:237-249). Dropping `self_tx` after this lets the writer
    // task's channel close so it half-closes the socket.
    let did_leave = {
        let mut g = lock(&state);
        g.on_connection_closed(conn_id, &session_key, now_ms())
    };
    if did_leave {
        schedule_shutdown_check(&state);
    }
    drop(self_tx);
}

/// Wire one accepted connection: split it, spawn its writer + reader, and register it.
pub(super) fn spawn_connection(
    conn_id: u64,
    stream: crate::transport::stream::BrokerStream,
    state: Arc<Mutex<BrokerState>>,
) {
    let (read_half, write_half) = stream.into_split();
    let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let close = Arc::new(Notify::new());
    tokio::spawn(writer_task(write_half, rx));
    {
        let mut g = lock(&state);
        g.add_connection(conn_id, close.clone());
    }
    tokio::spawn(reader_task(conn_id, read_half, tx, close, state));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::super::state::BrokerState;
    use super::super::test_support::{make_state, make_tx};
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Regression test for the framing.rs dossier item ("frames already reassembled before an
    /// oversize frame in the same `push()` call are discarded"): pi's reader delivers every complete
    /// frame found earlier in the same `data` chunk to `onMessage` synchronously, in order, BEFORE it
    /// discovers a later oversize length (`framing.ts:52-84`). Before this fix, `reader_task`'s
    /// `Err(_) => break` on `reader.push` discarded `FrameReadError::frames` entirely — a `register`
    /// frame reassembled earlier in the very same chunk as a trailing oversize header would never
    /// reach `handle_frame`, silently dropping a connection's registration. This test fails against
    /// that pre-fix behavior: `session_key` would stay `None` instead of naming `s1`.
    #[test]
    fn oversize_chunk_still_dispatches_frames_reassembled_earlier_in_the_same_chunk() {
        let state: Arc<Mutex<BrokerState>> = Arc::new(Mutex::new(make_state()));
        let mut session_key: Option<SessionKey> = None;
        let mut bucket = TokenBucket::new(now_ms());
        let self_tx = make_tx();

        let register_payload = json!({
            "type": "register",
            "sessionId": "s1",
            "session": {"cwd": "/tmp", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0}
        });
        let register_bytes = serde_json::to_vec(&register_payload).unwrap();
        let mut chunk = crate::transport::framing::encode_frame(&register_bytes);
        // Append a bogus trailing frame header declaring an over-cap length, in the SAME chunk.
        let bad_len = (crate::transport::framing::MAX_FRAME_BYTES as u32) + 1;
        chunk.extend_from_slice(&bad_len.to_be_bytes());

        let mut reader = FrameReader::new();
        let err = reader
            .push(&chunk)
            .expect_err("oversize declared length must error");
        assert_eq!(
            err.frames.len(),
            1,
            "the register frame reassembled before the oversize header must be preserved, not discarded"
        );

        for payload in &err.frames {
            let outcome =
                process_frame_payload(payload, 1, &self_tx, &state, &mut bucket, &mut session_key);
            assert!(
                outcome.keep_going,
                "a valid register frame must not itself trip a teardown"
            );
        }
        assert_eq!(
            session_key.as_ref().map(|k| k.id.as_str()),
            Some("s1"),
            "the preserved register frame must actually be dispatched to handle_frame, not discarded"
        );
    }
}
