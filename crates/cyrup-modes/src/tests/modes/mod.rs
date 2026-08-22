//! Integration tests for the non-interactive adapters (arch-11 §2.2; func-11 R-11-005/007/011…016).
//!
//! Each test builds a real wired [`AgentSession`] over a scripted `FauxProvider` in a tempdir and
//! drives one adapter into an in-memory sink, then asserts on the produced bytes — exactly how the
//! binary will drive them over real stdio.
//!
//! Split by concern: [`print_mode`] and [`json_mode`] cover the two one-shot adapters; the `rpc_*`
//! modules cover the bidirectional protocol — its verb surface ([`rpc_commands`]), its failure
//! envelopes ([`rpc_errors`]), its request deserialization ([`rpc_command_parsing`]), its bash
//! surface ([`rpc_bash`]), its extension-UI transport ([`rpc_ui_dialogs`], [`rpc_ui_effects`]),
//! contained extension faults ([`rpc_extension_errors`]) and the model registry ([`rpc_models`]).
//! Every fixture they share lives in [`support`].
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod support;

mod json_mode;
mod print_mode;
mod rpc_bash;
mod rpc_command_parsing;
mod rpc_commands;
mod rpc_errors;
mod rpc_extension_errors;
mod rpc_models;
mod rpc_ui_dialogs;
mod rpc_ui_effects;
