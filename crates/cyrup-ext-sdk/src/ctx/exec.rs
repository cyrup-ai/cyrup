//! The `exec` WIT import: a bounded run-capture-return one-shot. The long-lived duplex-pipe
//! counterpart is `proc`.

use crate::descriptor::ExecOptions;

use super::Ctx;

impl Ctx {
    /// Run a capability-scoped command (R-08-030). Denied unless the host granted the exec capability.
    pub fn exec(&self, cmd: &str, args: &[&str], opts: &ExecOptions) -> Result<ExecResult, String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            match crate::guest::bindings::cyrup::ext::exec::run(cmd, &argv, &opts_json) {
                Ok(r) => Ok(ExecResult {
                    code: r.code,
                    stdout: r.stdout,
                    stderr: r.stderr,
                    killed: r.killed,
                }),
                Err(e) => Err(e),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (cmd, args, opts_json);
            Err("exec unavailable on host target".into())
        }
    }
}

/// Result of [`Ctx::exec`] (Pi `ExecResult`, exec.ts:23-28). `killed` is true when the host
/// SIGTERMed, then (after a 5s grace period if still alive) SIGKILLed, the process GROUP on a
/// timeout/abort — Pi's exact `killProcess` escalation (exec.ts:52-63).
#[derive(Clone, Debug, Default)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub killed: bool,
}
