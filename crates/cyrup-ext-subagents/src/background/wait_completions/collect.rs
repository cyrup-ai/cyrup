//! [`collect_wait_completions`] — the three-rung resolution a `wait` reads its terminal
//! completions through.

use std::path::Path;

use crate::background::RunStatus;
use crate::background::result_index::{self, errno};
use crate::error::SubagentError;
use crate::identity::ResultFileName;

use super::project::{WaitCompletion, to_wait_completion};
use super::record::WaitCompletionStore;

/// Terminal payloads for the runs a wait covered.
///
/// pi `collectWaitCompletions` (`wait-completions.ts:160-213`). Read-only by contract: the watcher
/// owns notification and cleanup, and a payload is written atomically
/// ([`crate::background::atomic`]), so a direct read never observes a torn write. Nothing here
/// mints or touches a [`crate::background::result_index::ConsumablePayload`] — a wait must not be
/// able to destroy what it reads.
///
/// # Resolution order, per run
///
/// 1. **the in-process record** ([`WaitCompletionStore`]) — the watcher may already have consumed
///    and deleted the payload;
/// 2. **the session index** ([`result_index::result_payload_path_for_session_run`]) when the run
///    has a session, falling back to the public path — this is what makes a **staged** payload
///    visible to its owner and another session's payload invisible;
/// 3. **the public path alone** when the run has no session — an unattributed run is not indexed
///    anywhere, so there is nothing else to consult.
///
/// # Errors
///
/// A read fault that is not absence, any JSON parse failure, and any projection rejection. An
/// access-denied fault from the index read is recovered once via
/// [`result_index::fallback_result_payload_path_for_session_run`] before being surfaced.
pub async fn collect_wait_completions(
    terminal: &[RunStatus],
    store: &WaitCompletionStore,
    results_dir: &Path,
) -> Result<Vec<WaitCompletion>, SubagentError> {
    let mut out = Vec::with_capacity(terminal.len());

    for run in terminal {
        if let Some(recorded) = store.get(&run.run_id) {
            out.push(recorded);
            continue;
        }

        let file = ResultFileName::for_run(&run.run_id);
        let public = file.resolve_in(results_dir);
        let path = match run.session_id.as_ref() {
            // An unattributed run is not indexed anywhere; the public path is the only address.
            None => public.clone(),
            Some(session) => {
                match result_index::result_payload_path_for_session_run(
                    results_dir,
                    session,
                    &run.run_id,
                )
                .await
                {
                    Ok(found) => found.unwrap_or_else(|| public.clone()),
                    // A denied index read retries once against the STAGED location alone — the
                    // recovery path is still reachable because it is addressed directly rather
                    // than through the unreadable index directory.
                    Err(error) if errno::is_access_denied(&error) => {
                        result_index::fallback_result_payload_path_for_session_run(
                            results_dir,
                            session,
                            &run.run_id,
                        )
                        .await
                        .unwrap_or_else(|| public.clone())
                    }
                    Err(error) => return Err(SubagentError::Spawn(error)),
                }
            }
        };

        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
                    SubagentError::Spawn(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error,
                    ))
                })?;
                let completion = to_wait_completion(&value, &run.run_id).map_err(|error| {
                    SubagentError::Spawn(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error,
                    ))
                })?;
                out.push(completion);
            }
            Err(error) if errno::is_absent(&error) => {
                // The watcher may have consumed the file between the store check above and this
                // read: its drain loop runs independently of this one, so the in-process record
                // can appear in the meantime.
                if let Some(late) = store.get(&run.run_id) {
                    out.push(late);
                }
                // pi's third rung is `readCompletionReplay(resultsDir, runId, {sessionId})`. This
                // build has no durable-replay reader yet: until SCOPE_4 lands `read_completion_replay`
                // here, a run whose payload vanished and whose record expired contributes nothing.
            }
            Err(error) => return Err(SubagentError::Spawn(error)),
        }
    }

    Ok(out)
}
