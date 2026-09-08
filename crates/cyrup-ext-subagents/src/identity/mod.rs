//! Domain identities for session-scoped subagent work.
//!
//! A leaf module: it depends on nothing else in this crate except
//! [`crate::background::RunId`], performs no I/O, and is exhaustively unit-testable without a
//! tempdir, a runtime, or a mock.
//!
//! # Why these are types
//!
//! Every subagent run in a directory is written into roots keyed by **cwd**, never by session
//! (`background/artifact_roots.rs:281-284`), so every concurrent cyrup instance in that directory
//! sees every other instance's runs. What separates them is identity — and before this module the
//! crate carried that identity as bare `Option<String>`s, re-validated ad hoc at each use site.
//! The result was a class of bug that is invisible at the type level:
//!
//! | Invariant | Was | Failure it permitted | Now |
//! |---|---|---|---|
//! | a session id is non-empty | `Option<String>`, filtered at ≥3 sites | filters drift; `Some("")` reads as attributed | [`SessionId`] |
//! | owner id ≠ session id | both `String` | silent argument transposition in `owns(a, b)` | [`CompletionOwnerId`] |
//! | a path segment is encoded, bounded, portable | "call encode, then join" by convention | a raw session id — usually a *file path* — joined into a path | [`IndexSegment`] |
//! | a result file name is one component + `.json` | a runtime guard, duplicated 3× | traversal out of the results root | [`ResultFileName`] |
//!
//! Each type has exactly one fallible constructor and no `From<String>`, so the invariant holds by
//! construction rather than by every caller remembering it.
//!
//! # Serde is a construction path
//!
//! Every type here that appears in a persisted struct deserializes **through** its validating
//! constructor rather than via `#[serde(transparent)]`. On-disk data is written by other
//! processes — including older and newer builds — so deserialization is precisely where an
//! invariant would otherwise be bypassed. `Serialize` stays transparent; writing is not a trust
//! boundary and the on-disk shape must remain a bare JSON string.
//!
//! # Ports
//!
//! * [`SessionId`] — pi's `nonEmptyString(data.sessionId)` discipline, `result-files.ts:39-42`
//! * [`CompletionOwnerId`] / [`current_completion_owner_id`] — pi `shared/completion-owner.ts:10-14`
//! * [`IndexSegment`] — pi `runs/background/index-segment.ts` (whole file)
//! * [`ResultFileName`] — pi's traversal guard, `result-files.ts:260`, `:294`, `:307`
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs           facade + the invariant table
//! session_id.rs    SessionId — the launching session
//! owner_id.rs      CompletionOwnerId — the launching process, + its OnceLock mint
//! path_segment.rs  IndexSegment — encoding a value into ONE safe path component
//! result_name.rs   ResultFileName — the traversal guard, at parse
//! ```

mod owner_id;
mod path_segment;
mod result_name;
mod session_id;

pub use owner_id::{CompletionOwnerId, current_completion_owner_id};
pub use path_segment::IndexSegment;
pub use result_name::ResultFileName;
pub use session_id::SessionId;
