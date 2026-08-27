//! The `finishReason` table.

use super::*;

/// pi `google-generative-ai.ts:214-216` + `:269-273`: the raw finishReason names itself in the
/// terminal error. Gemini's real failure modes are all finish reasons, and before this they all
/// collapsed to the identical "An unknown error occurred".
#[test]
fn a_non_stop_finish_reason_names_itself_in_the_error() {
    for reason in [
        "SAFETY",
        "RECITATION",
        "PROHIBITED_CONTENT",
        "BLOCKLIST",
        "MALFORMED_FUNCTION_CALL",
    ] {
        let (stop, err) = map_stop_reason(reason);
        assert_eq!(stop, StopReason::Error, "{reason}");
        assert_eq!(
            err.as_deref(),
            Some(format!("Provider stopped with: {reason}").as_str()),
            "{reason} must be distinguishable from every other block reason"
        );
    }

    // The two non-error arms stay clean and carry no diagnostic.
    assert_eq!(map_stop_reason("STOP"), (StopReason::Stop, None));
    assert_eq!(map_stop_reason("MAX_TOKENS"), (StopReason::Length, None));
}
