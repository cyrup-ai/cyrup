//! In-crate unit tests (relocated from `crates/cyrup/tests/`).
//!
//! Cargo compiles every file under `tests/` into its own integration-test BINARY and process. The
//! files here are entirely in-process — they call the same library functions `main.rs` calls
//! (`build_inputs`, the provider-resolution fns, the `run_*_dispatch` entry points, the
//! first-time-setup gate) — so they belong with the library and compile under
//! `cargo check -p cyrup --all-targets`.
//!
//! The files that genuinely need the BINARY seam (`CARGO_BIN_EXE_cyrup`, real signals, a real
//! `git` process) stay in `crates/cyrup/tests/`: exit codes, stderr text and stdout/stderr
//! separation are only observable there. So does `tests/first_time_setup.rs`, for a different
//! reason: it mutates process-global env (`CYRUP_EXPERIMENTAL` / `PI_EXPERIMENTAL`) through
//! `unsafe { std::env::set_var }`, whose SAFETY note names its OWN test binary as the scope that
//! makes the mutation sound. This crate is `#![forbid(unsafe_code)]` — un-cancellable by any
//! inner `allow` — and the merged lib-test binary runs its tests on many threads, so the file
//! needs its own process both to compile and to stay sound.
//!
//! Assertions are unchanged from the integration-test originals; only the crate self-reference
//! moved (`cyrup::X` -> `crate::X`).

mod catalog_refresh_modes;
mod dispatch;
mod image_auto_resize_file_args;
mod image_bytecap;
mod install_package_dir;
mod models_json_resolution;
