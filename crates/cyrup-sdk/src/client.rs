//! [`Cyrup`] + [`CyrupBuilder`] — the embedder entry point.
//!
//! `Cyrup::builder()` configures the cross-session knobs and then `build_session(provider, config)`
//! assembles a wired [`Session`] over the [`cyrup_session_svc::SessionBuilder`] seam. The builder
//! adds **no behaviour**; it is a thin, stable construction surface.
//!
//! Advanced wiring — native built-in extensions, a custom credential store, settings overrides —
//! lives on the underlying [`SessionBuilder`]. Those APIs take types from `cyrup-ext`/`cyrup-config`,
//! which embedders pull in directly; reach them via [`CyrupBuilder::customize`] without this crate
//! re-exporting every internal type.

use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};

use crate::error::SdkResult;
use crate::handle::Session;

/// A customization applied to the underlying [`SessionBuilder`] before `build`.
type Customizer = Box<dyn FnOnce(SessionBuilder) -> SessionBuilder + Send>;

/// The SDK entry point. Call [`Cyrup::builder`] to start configuring an embedding.
///
/// # Examples
/// ```no_run
/// # use std::sync::Arc;
/// # async fn demo(
/// #     provider: Arc<dyn cyrup_provider::Provider>,
/// #     config: cyrup_sdk::SessionConfig,
/// # ) -> cyrup_sdk::SdkResult<()> {
/// use cyrup_sdk::Cyrup;
///
/// let session = Cyrup::builder().build_session(provider, config).await?;
/// let answer = session.run("hello").await?;
/// println!("{answer}");
/// # Ok(()) }
/// ```
pub struct Cyrup;

impl Cyrup {
    /// Start a [`CyrupBuilder`] with embedder-friendly defaults.
    ///
    /// # Examples
    /// ```
    /// let _builder = cyrup_sdk::Cyrup::builder();
    /// ```
    #[must_use]
    pub fn builder() -> CyrupBuilder {
        CyrupBuilder::default()
    }
}

/// Configures construction inputs, then builds a [`Session`] per provider + [`SessionConfig`].
///
/// A minimal embedding is just `Cyrup::builder().build_session(provider, config)`. For native
/// extensions / custom auth / settings stores, use [`CyrupBuilder::customize`] to reach the wrapped
/// [`SessionBuilder`].
///
/// # Examples
/// ```no_run
/// # use std::sync::Arc;
/// # async fn demo(
/// #     provider: Arc<dyn cyrup_provider::Provider>,
/// #     config: cyrup_sdk::SessionConfig,
/// # ) -> cyrup_sdk::SdkResult<()> {
/// // `customize` reaches the underlying SessionBuilder for advanced wiring, e.g.
/// // `b.with_native_extension(ext)` / `b.auth(store)` / `b.cli_settings(settings)`.
/// let session = cyrup_sdk::Cyrup::builder()
///     .customize(|b| b) // pass-through; real use calls a SessionBuilder method
///     .build_session(provider, config)
///     .await?;
/// # let _ = session;
/// # Ok(()) }
/// ```
#[derive(Default)]
pub struct CyrupBuilder {
    customizers: Vec<Customizer>,
}

impl CyrupBuilder {
    /// Apply a transformation to the underlying [`SessionBuilder`] just before it is built.
    ///
    /// This is the escape hatch for any [`SessionBuilder`] method whose argument types come from an
    /// internal crate (e.g. `with_native_extension`, `auth`, `settings_store`, `cli_settings`).
    /// Customizers run in registration order.
    ///
    /// # Examples
    /// ```
    /// // A pass-through customizer; real wiring calls a `SessionBuilder` method on `b`
    /// // (e.g. `with_native_extension`, `auth`, `settings_store`, `cli_settings`).
    /// let _builder = cyrup_sdk::Cyrup::builder().customize(|b| b);
    /// ```
    #[must_use]
    pub fn customize(
        mut self,
        f: impl FnOnce(SessionBuilder) -> SessionBuilder + Send + 'static,
    ) -> Self {
        self.customizers.push(Box::new(f));
        self
    }

    /// Assemble a wired [`Session`] over the given `provider` and `config`.
    ///
    /// Resolves settings + trust + auth + model, discovers resources, builds tools, opens/creates
    /// the session tree, assembles the system prompt, and loads any extensions registered via
    /// [`CyrupBuilder::customize`] — all via [`cyrup_session_svc::SessionBuilder`].
    ///
    /// # Errors
    /// Returns [`crate::SdkError`] if the underlying facade build fails (e.g. an unknown model
    /// pattern, an empty provider catalog, or a failing extension `init`).
    ///
    /// # Examples
    /// ```no_run
    /// # use std::sync::Arc;
    /// # async fn demo(
    /// #     provider: Arc<dyn cyrup_provider::Provider>,
    /// #     config: cyrup_sdk::SessionConfig,
    /// # ) -> cyrup_sdk::SdkResult<()> {
    /// let session = cyrup_sdk::Cyrup::builder().build_session(provider, config).await?;
    /// # let _ = session;
    /// # Ok(()) }
    /// ```
    pub async fn build_session(
        self,
        provider: Arc<dyn Provider>,
        config: SessionConfig,
    ) -> SdkResult<Session> {
        let mut builder = SessionBuilder::new(provider, config);
        for customize in self.customizers {
            builder = customize(builder);
        }
        Ok(Session::new(builder.build().await?))
    }
}
