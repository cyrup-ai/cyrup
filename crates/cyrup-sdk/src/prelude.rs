//! Ergonomic re-exports for embedders: `use cyrup_sdk::prelude::*;`.
//!
//! Pulls in the entry point ([`Cyrup`]/[`CyrupBuilder`]/[`Session`]), the construction inputs
//! ([`SessionConfig`]/[`SessionTarget`]), the load-bearing wire types
//! ([`AgentSessionEvent`]/[`UserInput`]/[`EventStream`]), and the error type — so a typical
//! embedding needs only this one glob import.
//!
//! # Examples
//! ```no_run
//! use cyrup_sdk::prelude::*;
//!
//! # async fn demo(provider: std::sync::Arc<dyn cyrup_provider::Provider>) -> SdkResult<()> {
//! let config = SessionConfig::new(".", "/tmp/agent");
//! let session = Cyrup::builder().build_session(provider, config).await?;
//! let answer = session.run("hi").await?;
//! # let _ = answer;
//! # Ok(()) }
//! ```

pub use crate::client::{Cyrup, CyrupBuilder};
pub use crate::error::{SdkError, SdkResult};
pub use crate::handle::Session;

pub use crate::{
    AgentSessionEvent, AgentSessionServices, EventStream, InputSource, PromptAccepted,
    SessionConfig, SessionServiceError, SessionTarget, StreamingBehavior, UserInput,
};
