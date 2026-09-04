//! Error bodies.

use super::*;

#[test]
fn usage_limit_bodies_get_the_friendly_message() {
    // pi parseErrorResponse (:1533-1558) + `info.friendlyMessage || info.message` (:446).
    let now = 1_700_000_000_000i64;
    let resets_at = (now / 1000) + 3600;
    let raw = json!({
        "error": {
            "code": "usage_limit_reached",
            "plan_type": "Plus",
            "resets_at": resets_at,
        }
    })
    .to_string();
    assert_eq!(
        parse_error_response(429, &raw, now),
        "You have hit your ChatGPT usage limit (plus plan). Try again in ~60 min."
    );

    // No plan and no reset time: the trimmed bare sentence.
    let raw = json!({ "error": { "code": "usage_not_included" } }).to_string();
    assert_eq!(
        parse_error_response(403, &raw, now),
        "You have hit your ChatGPT usage limit."
    );

    // Any 429 gets the friendly message even with an unrelated code.
    let raw = json!({ "error": { "code": "slow_down", "message": "chill" } }).to_string();
    assert_eq!(
        parse_error_response(429, &raw, now),
        "You have hit your ChatGPT usage limit."
    );
}

#[test]
fn non_limit_bodies_surface_the_provider_message() {
    let now = 1_700_000_000_000i64;
    // MIRROR: without the limit code and without a 429, `err.message` is what surfaces.
    let raw = json!({ "error": { "code": "invalid_request", "message": "bad model" } }).to_string();
    assert_eq!(parse_error_response(400, &raw, now), "bad model");
    // Unparseable body: the raw text.
    assert_eq!(
        parse_error_response(500, "upstream boom", now),
        "upstream boom"
    );
    // Empty body: `raw || statusText || "Request failed"`.
    assert_eq!(parse_error_response(500, "", now), "Request failed");
}
