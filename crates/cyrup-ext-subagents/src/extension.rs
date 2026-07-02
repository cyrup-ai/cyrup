//! `NativeExtension` impl: init/on_event/execute_command (arch-SA §3.2/§6.8).
//!
//! Registered exactly once, at session-service construction, via the same
//! `SessionFactory::with_native_extension` path arch-08 §3.1 documents for any native built-in —
//! always-loaded, never project-trust-gated (arch-SA §6.1).
