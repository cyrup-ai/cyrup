//! Model-id capability probes.

use super::*;

#[test]
fn model_id_detection() {
    assert!(is_gemini3_pro(&model_with("gemini-3-pro-preview", true)));
    assert!(is_gemini3_pro(&model_with("gemini-3.1-pro", true)));
    assert!(!is_gemini3_pro(&model_with("gemini-2.5-pro", true)));
    assert!(is_gemini3_flash(&model_with(
        "gemini-3-flash-preview",
        true
    )));
    assert!(is_gemini3_flash(&model_with("gemini-flash-latest", true)));
    assert!(is_gemma4(&model_with("gemma-4-2b", true)));
    assert_eq!(gemini_major_version("gemini-2.5-pro"), Some(2));
    assert_eq!(gemini_major_version("gemini-3-pro-preview"), Some(3));
    assert!(supports_multimodal_function_response(
        "gemini-3-pro-preview"
    ));
    assert!(!supports_multimodal_function_response("gemini-2.5-pro"));
    assert!(supports_multimodal_function_response("claude-opus-4-5"));
}
