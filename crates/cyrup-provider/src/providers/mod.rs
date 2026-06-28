//! Concrete vendor providers (arch-01 §5). Each is a [`crate::wire::WireProvider`] = a catalog + an
//! auth strategy + an api mapping over the shared [`crate::api::ApiRegistry`].

pub mod together;

pub use together::{
    together_auth, together_models, together_provider, together_provider_with, TOGETHER_BASE_URL,
};
