//! URL.

use super::*;

#[test]
fn codex_url_completes_without_doubling_segments() {
    // pi resolveCodexUrl (:637-643).
    assert_eq!(
        resolve_codex_url(""),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("   "),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://example.test/backend-api/"),
        "https://example.test/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://example.test/backend-api/codex"),
        "https://example.test/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://example.test/backend-api/codex/responses///"),
        "https://example.test/backend-api/codex/responses"
    );
}

