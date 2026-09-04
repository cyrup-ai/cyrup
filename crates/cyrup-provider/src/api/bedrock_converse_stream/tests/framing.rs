//! Event-stream framing.

use super::*;

#[test]
fn the_event_stream_decoder_handles_split_and_coalesced_chunks() {
    let bytes = [
        event("messageStart", "{\"role\":\"assistant\"}"),
        event(
            "contentBlockDelta",
            "{\"contentBlockIndex\":0,\"delta\":{\"text\":\"hi\"}}",
        ),
    ]
    .concat();

    // One byte at a time — the decoder must never mis-frame.
    let mut dec = EventStreamDecoder::default();
    let mut seen = Vec::new();
    for byte in &bytes {
        dec.push(std::slice::from_ref(byte));
        while let Some(f) = dec.next_frame().unwrap() {
            seen.push(f.header(":event-type").unwrap());
        }
    }
    assert_eq!(seen, vec!["messageStart", "contentBlockDelta"]);

    // Both frames in one chunk.
    let mut dec = EventStreamDecoder::default();
    dec.push(&bytes);
    assert_eq!(
        dec.next_frame().unwrap().unwrap().header(":event-type"),
        Some("messageStart".to_string())
    );
    assert_eq!(
        dec.next_frame().unwrap().unwrap().header(":event-type"),
        Some("contentBlockDelta".to_string())
    );
    assert!(dec.next_frame().unwrap().is_none());
}

#[test]
fn a_corrupted_frame_is_rejected_by_its_checksum() {
    let mut bytes = event("messageStart", "{\"role\":\"assistant\"}");
    let last = bytes.len() - 5;
    bytes[last] ^= 0xFF;
    let mut dec = EventStreamDecoder::default();
    dec.push(&bytes);
    assert!(dec.next_frame().is_err());
}

#[test]
fn non_string_header_values_do_not_desynchronise_the_walk() {
    let mut header_bytes = Vec::new();
    // A timestamp header (type 8), then the string header we care about.
    header_bytes.push(4u8);
    header_bytes.extend_from_slice(b"when");
    header_bytes.push(8);
    header_bytes.extend_from_slice(&0i64.to_be_bytes());
    header_bytes.push(11u8);
    header_bytes.extend_from_slice(b":event-type");
    header_bytes.push(7);
    header_bytes.extend_from_slice(&(8u16).to_be_bytes());
    header_bytes.extend_from_slice(b"metadata");

    let parsed = parse_event_headers(&header_bytes).unwrap();
    assert_eq!(
        parsed.get(":event-type").map(String::as_str),
        Some("metadata")
    );
    assert!(!parsed.contains_key("when"));
}
