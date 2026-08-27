//! The `openai-completions` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! One [`ApiImpl`](crate::api::ApiImpl) speaking the OpenAI Chat Completions streaming API
//! (`POST {baseUrl}/chat/
//! completions`, SSE chunks with `choices[].delta.{content,reasoning_content,tool_calls[]}` +
//! `finish_reason`, and a final `usage` chunk via `stream_options.include_usage=true`). Shared by
//! every OpenAI-compatible provider (openai, together, groq, …) — they differ only in base URL,
//! auth, and catalog (R-01-007). Ports Pi's proven `openai-completions.ts` encoder/decoder.
//!
//! Wire JSON uses the vendor's own field names (snake_case), NOT the cyrup camelCase convention.

mod blocks;
mod cache;
mod convert;
mod decode;
mod deltas;
mod driver;
mod finalize;
mod headers;
mod params;
mod reasoning;
mod tools;
mod transform;

#[cfg(test)]
mod tests;

pub use driver::{OpenAiCompletionsApi, factory};

pub(crate) use params::apply_sampling_params;
pub(crate) use transform::{transform_messages_with, transform_messages_with_source};
// Test-only crate re-exports: `build_body` is the providers' body fixture and `decode_stream` is
// what `api::truncation_parity` drives directly (both `#[cfg(test)]` call sites).
#[cfg(test)]
pub(crate) use decode::decode_stream;
#[cfg(test)]
pub(crate) use params::build_body;
