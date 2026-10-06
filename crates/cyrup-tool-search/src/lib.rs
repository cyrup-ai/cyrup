//! `cyrup-tool-search` — pi's `tool_search` tool (gap-analysis `TOOL-052`).
//!
//! `tool_search` searches the tools that are registered but not declared to the model (`codemode`
//! and `deferred` exposure), ranks them with BM25 and loads the matches into the active tool set, so
//! the next model call declares them. It is the only way a `deferred` tool, such as a search-mode MCP
//! tool, reaches the model.
//!
//! | module | upstream @v1.0.1 |
//! |---|---|
//! | [`input`] | `extensions/tool-search/tool.ts:159-163,234-236` (schema, the two validations) |
//! | [`search`] | `extensions/tool-search/tool.ts:187-216` (`isSearchable`, `searchAndLoad`, the result text) |
//! | [`tool`] | `extensions/tool-search/tool.ts:169-247` (the tool definition) |
//! | [`extension`] | `extensions/tool-search/index.ts` |
//!
//! The ranker, the search document and the tokenizer are `cyrup_codemode::rank`
//! (`extensions/tool-search/tool.ts:25-157`); `codemode`'s `searchTools()` shares them.

#![forbid(unsafe_code)]

pub mod extension;
pub mod input;
pub mod search;
pub mod tool;

pub use extension::{EXTENSION_ID, ToolSearchExtension};
pub use tool::{TOOL_SEARCH_DESCRIPTION, TOOL_SEARCH_TOOL_NAME, ToolSearchTool};

#[cfg(test)]
mod tests;
