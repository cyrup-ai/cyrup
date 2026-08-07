//! Context-overflow detection (1:1 with Pi `utils/overflow.ts`).
//!
//! Classifies whether a failed (or silently-truncated) assistant turn was caused by the input
//! exceeding the model's context window. Compaction depends on this signal. The provider-specific
//! regex set, the non-overflow exclusions, and the three detection cases below are a faithful port
//! of Pi `overflow.ts:37-161` (pi v0.83.0).

use crate::utils::regexlite::Regex;
use cyrup_core::{AssistantMessage, StopReason};
use std::sync::OnceLock;

/// Provider-specific overflow error patterns (Pi `OVERFLOW_PATTERNS`, overflow.ts:37-62). Each entry
/// is the exact Pi source pattern, in Pi's order (the `/i` flag is implicit — [`Regex`] is always
/// case-insensitive).
const OVERFLOW_PATTERNS: &[&str] = &[
    r"prompt is too long",                    // Anthropic token overflow
    r"request_too_large",                     // Anthropic request byte-size overflow (HTTP 413)
    r"input is too long for requested model", // Amazon Bedrock
    r"exceeds the context window",            // OpenAI (Completions & Responses API)
    r"exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))", // OpenAI-compatible proxies (LiteLLM)
    r"input token count.*exceeds the maximum", // Google (Gemini)
    r"maximum prompt length is \d+",           // xAI (Grok)
    r"reduce the length of the messages",      // Groq
    r"maximum context length is \d+ tokens",   // OpenRouter (most backends)
    r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?", // OpenRouter/Poolside
    r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)", // Together AI
    r"exceeds the limit of \d+",           // GitHub Copilot
    r"exceeds the available context size", // llama.cpp server
    r"greater than the context length",    // LM Studio
    r"context window exceeds limit",       // MiniMax
    r"exceeded model token limit",         // Kimi For Coding
    r"too large for model with \d+ maximum context length", // Mistral
    r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?", // DS4 server
    r"model_context_window_exceeded",      // z.ai non-standard finish_reason as error text
    r"prompt too long; exceeded (?:max )?context length", // Ollama explicit overflow error
    r"range of input length should be",    // DashScope / Qwen Token Plan
    r"context[_ ]length[_ ]exceeded",      // Generic fallback
    r"too many tokens",                    // Generic fallback
    r"token limit exceeded",               // Generic fallback
    r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)", // Cerebras: 400/413 with no body
];

/// Patterns that mark an error as NON-overflow (e.g. throttling / rate-limit) even if it also
/// matches an overflow pattern (Pi `NON_OVERFLOW_PATTERNS`, overflow.ts:70-74).
const NON_OVERFLOW_PATTERNS: &[&str] = &[
    r"^(Throttling error|Service unavailable):", // AWS Bedrock non-overflow (formatBedrockError prefixes)
    r"rate limit",                               // Generic rate limiting
    r"too many requests",                        // Generic HTTP 429 style
];

fn overflow_regexes() -> &'static [Regex] {
    static CELL: OnceLock<Vec<Regex>> = OnceLock::new();
    CELL.get_or_init(|| OVERFLOW_PATTERNS.iter().map(|p| Regex::new(p)).collect())
}

fn non_overflow_regexes() -> &'static [Regex] {
    static CELL: OnceLock<Vec<Regex>> = OnceLock::new();
    CELL.get_or_init(|| {
        NON_OVERFLOW_PATTERNS
            .iter()
            .map(|p| Regex::new(p))
            .collect()
    })
}

/// Check if an assistant message represents a context-overflow error (Pi `isContextOverflow`,
/// overflow.ts:132-161).
///
/// Handles three cases, in order:
/// 1. **Error-based** — `stopReason == error` with a message matching an overflow pattern (and not a
///    non-overflow exclusion).
/// 2. **Silent overflow** (z.ai style) — a successful (`stop`) turn whose `input + cacheRead`
///    exceeds `context_window`.
/// 3. **Length-stop overflow** (Xiaomi MiMo style) — `stopReason == length` with zero output and
///    `input + cacheRead` filling ≥ 99% of `context_window`.
///
/// `context_window` is `None` when the caller does not know it; cases 2 and 3 then never fire
/// (matching Pi's optional `contextWindow` parameter).
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    // Case 1: Check error message patterns.
    if let (StopReason::Error, Some(error_message)) =
        (message.stop_reason, message.error_message.as_deref())
    {
        let is_non_overflow = non_overflow_regexes()
            .iter()
            .any(|p| p.is_match(error_message));
        if !is_non_overflow && overflow_regexes().iter().any(|p| p.is_match(error_message)) {
            return true;
        }
    }

    // Case 2: Silent overflow (z.ai style) — successful but usage exceeds context.
    if let Some(window) = context_window {
        if message.stop_reason == StopReason::Stop {
            let input_tokens = message.usage.input + message.usage.cache_read;
            if input_tokens > window {
                return true;
            }
        }

        // Case 3: Length-stop overflow (Xiaomi MiMo style) — server truncates oversized input to
        // fit the context window, leaving no room for output.
        if message.stop_reason == StopReason::Length && message.usage.output == 0 {
            let input_tokens = message.usage.input + message.usage.cache_read;
            // `>= context_window * 0.99` without floating point: input*100 >= window*99.
            if (input_tokens as u128) * 100 >= (window as u128) * 99 {
                return true;
            }
        }
    }

    false
}

/// Get the overflow patterns (Pi `getOverflowPatterns`, overflow.ts:166-168) — for testing.
pub fn overflow_patterns() -> &'static [&'static str] {
    OVERFLOW_PATTERNS
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use cyrup_core::{ProviderId, Usage};

    fn err(message: &str) -> AssistantMessage {
        AssistantMessage::errored(
            ProviderId::from("anthropic"),
            "claude",
            None,
            StopReason::Error,
            message,
        )
    }

    fn ok_with_usage(
        stop: StopReason,
        input: u64,
        cache_read: u64,
        output: u64,
    ) -> AssistantMessage {
        let mut m = AssistantMessage::errored(ProviderId::from("zai"), "glm", None, stop, "");
        m.error_message = None;
        m.usage = Usage {
            input,
            cache_read,
            output,
            ..Usage::default()
        };
        m
    }

    /// Every documented provider error string (overflow.ts:9-33) is detected.
    #[test]
    fn detects_each_provider_overflow_message() {
        let cases = [
            "prompt is too long: 213462 tokens > 200000 maximum", // Anthropic
            "413 {\"error\":{\"type\":\"request_too_large\",\"message\":\"Request exceeds the maximum size\"}}", // Anthropic 413
            "Your input exceeds the context window of this model", // OpenAI
            "Requested token count exceeds the model's maximum context length of 131072 tokens", // LiteLLM
            "Input length (265330) exceeds model's maximum context length (262144).", // OpenAI-compatible
            "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)", // Google
            "This model's maximum prompt length is 131072 but the request contains 537812 tokens", // xAI
            "Please reduce the length of the messages or completion", // Groq
            "This endpoint's maximum context length is 8192 tokens. However, you requested about 9000 tokens", // OpenRouter
            "Input length 5000 exceeds the maximum allowed input length of 4096 tokens.", // OpenRouter/Poolside
            "The input (5000 tokens) is longer than the model's context length (4096 tokens).", // Together
            "the request exceeds the available context size, try increasing it", // llama.cpp
            "tokens to keep from the initial prompt is greater than the context length", // LM Studio
            "prompt token count of 9000 exceeds the limit of 8192", // GitHub Copilot
            "invalid params, context window exceeds limit",         // MiniMax
            "Your request exceeded model token limit: 8192 (requested: 9000)", // Kimi
            "Prompt contains 9000 tokens ... too large for model with 8192 maximum context length", // Mistral
            "input is too long for requested model", // Bedrock
            "400 (no body)",                         // Cerebras
            "413 status code (no body)",             // Cerebras
        ];
        for c in cases {
            assert!(
                is_context_overflow(&err(c), None),
                "should detect overflow: {c}"
            );
        }
    }

    /// Non-overflow errors are excluded even when they share overflow wording.
    #[test]
    fn excludes_non_overflow_errors() {
        // Bedrock throttling shares "too many tokens" wording but must NOT be overflow.
        let throttle =
            err("ThrottlingException: Too many tokens, please wait before trying again.");
        // The NON_OVERFLOW exclusion is anchored to a human-readable prefix; this raw form has no
        // such prefix, so verify the prefixed form (formatBedrockError output) is excluded.
        let prefixed = err("Throttling error: Too many tokens, please wait before trying again.");
        assert!(!is_context_overflow(&prefixed, None));
        // And generic rate-limit text never counts as overflow.
        assert!(!is_context_overflow(
            &err("rate limit exceeded, too many tokens"),
            None
        ));
        // The bare throttle (no prefix) DOES match "too many tokens" — Pi's behavior; documented.
        assert!(is_context_overflow(&throttle, None));
    }

    /// Case 2: silent overflow when input+cacheRead exceeds the context window on a `stop` turn.
    #[test]
    fn detects_silent_overflow() {
        let m = ok_with_usage(StopReason::Stop, 200_000, 5_000, 10);
        assert!(is_context_overflow(&m, Some(199_000)));
        assert!(!is_context_overflow(&m, Some(300_000)));
        assert!(!is_context_overflow(&m, None)); // needs a known window
    }

    /// Case 3: length-stop with zero output filling ≥99% of the window.
    #[test]
    fn detects_length_stop_overflow() {
        let m = ok_with_usage(StopReason::Length, 99_000, 1_000, 0);
        assert!(is_context_overflow(&m, Some(100_000))); // 100k >= 99k
        let with_output = ok_with_usage(StopReason::Length, 100_000, 0, 5);
        assert!(!is_context_overflow(&with_output, Some(100_000))); // output > 0 → not this case
    }

    #[test]
    fn clean_message_is_not_overflow() {
        assert!(!is_context_overflow(
            &err("the model refused the request"),
            Some(200_000)
        ));
    }
}
