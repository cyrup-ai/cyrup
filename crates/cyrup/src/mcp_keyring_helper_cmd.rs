//! `__mcp-keyring-helper` — the hidden, never-user-facing CLI subcommand that runs one MCP
//! credential keyring operation inside a freshly created session keyring (MCP-260; a 1:1 analog of
//! the `__intercom-broker` and `__subagent-runner` hops, `intercom_broker_cmd.rs` /
//! `subagent_runner_cmd.rs`).
//!
//! The parent side is already live in production: whenever an MCP OAuth credential read/write/remove
//! fails on Linux against a **revoked** session keyring,
//! [`cyrup_mcp::credentials::McpAuthStore`]'s recovery arm builds a
//! `LinuxKeyringRecoveryStore` and re-execs `current_exe()` as
//! `<keyctl> session - <cyrup> __mcp-keyring-helper`, handing the request over as one line of JSON
//! on stdin and reading one line of JSON back on stdout. `keyctl session -` creates an *anonymous*
//! session keyring and execs its remaining argv inside it, which is the only way a process attached
//! to a revoked keyring can perform a keyring call at all. This module recognizes that argv and
//! hands straight to [`cyrup_mcp::credentials::run_keyring_helper`], whose exit code it returns
//! unchanged (**1 on every error reply**, so the parent's rung-2 diagnostic keeps winning).
//!
//! [`dispatch`] is **synchronous**, unlike its two siblings: the helper is one blocking stdin read,
//! one keychain call and one stdout line, and [`cyrup_mcp::credentials::run_keyring_helper`] is a
//! plain `fn(&mut impl Read, &mut impl Write) -> i32`. There is no reactor work to do and nothing to
//! await.
//!
//! Carried verbatim from that function's contract, because it is what makes this dispatch correct
//! where it sits in `main()`: *it performs exactly one keyring operation and exits. It never reads
//! config, never touches the network, never logs a secret, and its stdout is exactly one line of
//! JSON. This is the one code path in the crate that must not initialise the cache, the config or
//! tracing.* Hence `main()` returns from the pre-dispatch match **above**
//! `bootstrap::install_bootstrap_http_proxy()` and above `predispatch::run_predispatch` — anything
//! initialised before the helper answers is either a secret-leaking log line or a stray byte on the
//! stdout the parent is parsing as JSON.
//!
//! Never advertised to users: not listed in `--help`, not one of `crate::subcommands::SUBCOMMANDS`,
//! and dispatched from `main()` BEFORE any user-facing arg leniency/clap parsing runs — mirroring
//! the `__intercom-broker` and `__subagent-runner` pre-dispatches' placement.
//!
//! # [CYRUP-DELTA] (SEAM-109) — an argv verb has no upstream counterpart at all
//!
//! **pi has NO argv verbs**; see [`crate::subagent_runner_cmd`]'s delta for the check that
//! establishes it (`git -C pi grep -nE 'argv\[2\]|process\.argv' v0.83.0` → nothing, and
//! `rpc-entry.ts` / `bun/cli.ts` are separate entry points, not verbs).
//!
//! **The mechanism this replaces** is `pi-mcp-adapter` v2.25.0's `mcp-keyring-helper.cjs` (89
//! lines): a SEPARATE SCRIPT handed to an interpreter, resolved as `./mcp-keyring-helper.cjs`
//! against `import.meta.url` and spawned as `keyctl session - <node> <helperPath>` — with
//! `PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE` naming the interpreter. As with the broker and the
//! subagent runner, upstream's selector is a script path, so the child needs no verb on the agent
//! binary. cyrup is one compiled binary with no interpreter to hand a script to, so the helper
//! PROCESS — which is the load-bearing part: a *separate* process, because `keyctl session -` can
//! only take effect across an `exec` — is re-exec'd out of `current_exe()` under a reserved argv
//! token instead. The `keyctl session -` argv shape, the one-line-JSON protocol, the 1 MiB cap, the
//! validation order and the exit codes are ported literally; only the SELECTOR differs, which is
//! why `…_KEYRING_RECOVERY_NODE` has no port and `…_KEYRING_RECOVERY_HELPER` now names a **program**
//! rather than a script ([`cyrup_mcp::credentials::KEYRING_RECOVERY_HELPER_ENV`]).
//!
//! **Deliberately undocumented:** `__`-prefixed, absent from `--help` and
//! `crate::subcommands::SUBCOMMANDS`, matched only as an exact `argv[1]` ([`is_selected`]).
//! Undiscoverable rather than absent — recorded so the invented surface is KNOWN, not assumed parity.

/// The literal `argv[1]` token identifying this internal subcommand. Re-exported from the crate that
/// spawns it ([`cyrup_mcp::credentials::KEYRING_HELPER_SUBCOMMAND`]) rather than restated, because
/// here the two sides are in a caller/callee relationship through a *public* constant: the recovery
/// store appends this exact token to `current_exe()`, so a second literal could only ever drift.
pub const SUBCOMMAND: &str = cyrup_mcp::credentials::KEYRING_HELPER_SUBCOMMAND;

/// Returns `true` if `argv` (the process's own args, *including* `argv[0]`/the binary name at index 0,
/// matching [`std::env::args`]'s shape) selects this internal subcommand.
#[must_use]
pub fn is_selected(argv: &[String]) -> bool {
    argv.get(1).map(String::as_str) == Some(SUBCOMMAND)
}

/// Run the one keyring operation to completion and return the process exit code `main` should use.
///
/// Synchronous by design (see the module docs): the whole body is one stdin drain, one keychain
/// call and one stdout line. Nothing is written to stderr and nothing is logged — stdout carries
/// exactly one line of JSON, which is the parent's entire channel.
#[must_use]
pub fn dispatch() -> i32 {
    cyrup_mcp::credentials::run_keyring_helper(&mut std::io::stdin(), &mut std::io::stdout())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn is_selected_matches_the_exact_internal_subcommand_token() {
        assert_eq!(SUBCOMMAND, "__mcp-keyring-helper");
        assert!(is_selected(&["cyrup".to_string(), "__mcp-keyring-helper".to_string()]));
        assert!(!is_selected(&["cyrup".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "__intercom-broker".to_string()]));
        assert!(!is_selected(&["cyrup".to_string(), "--help".to_string()]));
        assert!(!is_selected(&[]));
    }
}
