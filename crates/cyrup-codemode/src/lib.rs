//! `cyrup-codemode` — the engine-independent half of pi's `codemode` (gap-analysis area 18,
//! `docs/gap-analysis/18-pi-codemode.md`).
//!
//! `codemode` lets a model write one JavaScript program that calls tools instead of making one tool
//! call per turn. The JavaScript engine is a separate crate's concern; everything a script's author
//! and the model-facing description need *before* an engine runs lives here, and none of it knows
//! an engine exists:
//!
//! | module | upstream @v1.0.1 | ledger |
//! |---|---|---|
//! | [`identifier`] | `packages/codemode/src/identifier.ts` | CODE-004 |
//! | [`source`] | `packages/codemode/src/source.ts` | CODE-003 |
//! | [`declarations`] | `packages/codemode/src/declarations.ts` | CODE-004 |
//! | [`rank`] | `packages/coding-agent/src/extensions/tool-search/tool.ts` | CODE-010 |
//! | [`discovery`] | `packages/coding-agent/src/extensions/codemode/execute.ts:437-517` | CODE-010 |
//! | [`output`] | `packages/coding-agent/src/extensions/codemode/execute.ts:232-300` | CODE-012 |
//! | [`types`] | `packages/codemode/src/types.ts` | CODE-004 |
//!
//! # Byte parity with JavaScript
//!
//! The declaration text and the ranker's input are compared with upstream byte for byte, and
//! upstream computes them with JavaScript string and number semantics (UTF-16 lengths, the
//! `String.prototype.trim` whitespace set, `Array.prototype.sort`'s UTF-16 code-unit order,
//! `JSON.stringify`'s number format, own-key order with integer-like keys first). Those semantics
//! are reproduced once, in [`js`], and every module goes through it.

#![forbid(unsafe_code)]

pub mod declarations;
pub mod discovery;
pub mod identifier;
pub mod js;
pub mod output;
pub mod rank;
pub mod source;
pub mod types;
