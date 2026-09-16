//! The wait-subscription reference set — pi `parseWaitRunIds` (`:307-316`). **Fail-closed.**
//!
//! # The coupling this closes
//!
//! [`crate::background::wait_subscriptions`]'s own module doc named this task as the owner:
//!
//! > *"Upstream's async retention reaper reads this directory: `async-retention.ts:307-316`'s
//! > `parseWaitRunIds` protects a run referenced by a live subscription from being reaped,
//! > re-read before each destructive action, and skips the whole pass as
//! > `"wait-references-unknown"` when any record is unparseable. cyrup has no `async-retention.ts`
//! > port, so there is nothing to wire this into. Whoever ports the reaper owns the coupling."*
//!
//! This is the reader half. The re-read before each destructive action (`:794`, `:818`) and the
//! pass-level `wait-references-unknown` abort (`:752-755`) are sweep control flow and belong to
//! SCOPE_14; this function's `None` IS upstream's `safe: false`, so part B's abort is a `match`
//! and not a re-derivation.
//!
//! # Why the DIRECTORY and never `WaitSubscriptionManager::armed()`
//!
//! `armed()` is narrowed to the current session by construction, and the async root is SHARED
//! (see [`crate::background::run_artifact_roots`]). Reading the in-process set would leave
//! another instance's waited-on run unprotected in a root both instances can delete from. The
//! manager itself reads the directory back for exactly that reason.

use std::collections::BTreeSet;
use std::path::Path;

use crate::background::RunId;
use crate::background::result_index::errno;
use crate::background::wait_subscriptions::parse_record;
use crate::identity::ResultFileName;

/// pi `parseWaitRunIds` (`:307-316`) — every run id named by an armed subscription record.
///
/// `None` is upstream's `{ runIds, safe: false }`: **one** unparseable `*.json` record makes the
/// whole set unsafe, and the caller must abort the pass rather than act on a partial set. That is
/// fail-closed by design — an unreadable subscription might be protecting any run in the root, so
/// the only sound response is to delete nothing.
///
/// A missing or unlistable directory is `Some(empty)`, not `None`: pi's `listDir` (`:130-137`)
/// returns `[]` for `ENOENT` and the pass proceeds. cyrup widens that to
/// [`errno::is_ignorable_listing_error`], the crate's one enumeration policy, so a
/// permission-denied subscriptions directory reads as "no subscriptions" exactly as every other
/// listing in this crate does.
///
/// [CYRUP-DELTA] upstream validates only `typeof record.runId === "string" && typeof
/// record.expiresAt === "number"`. cyrup routes the bytes through
/// [`crate::background::wait_subscriptions::parse_record`], the one parser for this
/// format, which additionally rejects an unknown version, a malformed token, an empty session and
/// an unknown target kind. Every extra rejection resolves to `None` → abort the pass → delete
/// nothing, which is strictly the safe direction.
pub async fn wait_run_ids(dir: &Path) -> Option<BTreeSet<RunId>> {
    let mut run_ids = BTreeSet::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if errno::is_ignorable_listing_error(&error) => return Some(run_ids),
        // pi `listDir` rethrows a non-ENOENT listing error, which aborts the whole pass. cyrup has
        // no error channel here, and the equivalent outcome — do nothing destructive — is `None`.
        Err(_) => return None,
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(error) if errno::is_ignorable_listing_error(&error) => break,
            Err(_) => return None,
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.ends_with(ResultFileName::EXTENSION) {
            continue;
        }
        match entry.file_type().await {
            Ok(file_type) if file_type.is_file() => {}
            // `entry.isFile()` is false for anything else, including an unreadable type.
            _ => continue,
        }
        let Ok(bytes) = tokio::fs::read(entry.path()).await else {
            // pi's `readJson` catch → `undefined` → `safe: false` (`:312`).
            return None;
        };
        // `?` here IS pi `:312`'s `return { runIds, safe: false }`: one unparseable record aborts
        // the whole read, discarding the partial set rather than handing it back.
        run_ids.insert(parse_record(&bytes)?.run_id);
    }
    Some(run_ids)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    const TOKEN_A: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const TOKEN_B: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3302";

    fn record_json(token: &str, run_id: &str) -> String {
        format!(
            r#"{{"version":1,"token":"{token}","sessionId":"s-1","targetKind":"async",
               "runId":"{run_id}","requestedId":"{run_id}","createdAt":10,"expiresAt":99}}"#
        )
    }

    #[tokio::test]
    async fn an_absent_subscriptions_directory_is_an_empty_but_safe_set() {
        let temp = tempfile::tempdir().unwrap();
        let ids = wait_run_ids(&temp.path().join("nope")).await;
        assert_eq!(ids, Some(BTreeSet::new()));
    }

    #[tokio::test]
    async fn armed_records_contribute_their_run_ids() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(
            temp.path().join(format!("{TOKEN_A}.json")),
            record_json(TOKEN_A, "run-a"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            temp.path().join(format!("{TOKEN_B}.json")),
            record_json(TOKEN_B, "run-b"),
        )
        .await
        .unwrap();
        // Non-`.json` files are not records and are ignored (pi `:310`).
        tokio::fs::write(temp.path().join("notes.txt"), "ignored")
            .await
            .unwrap();

        let ids = wait_run_ids(temp.path()).await.unwrap();
        assert_eq!(
            ids.iter().map(RunId::as_str).collect::<Vec<_>>(),
            vec!["run-a", "run-b"]
        );
    }

    #[tokio::test]
    async fn wait_subscription_run_ids_fail_closed_on_an_unparseable_record() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(
            temp.path().join(format!("{TOKEN_A}.json")),
            record_json(TOKEN_A, "run-a"),
        )
        .await
        .unwrap();
        tokio::fs::write(temp.path().join("broken.json"), "{}")
            .await
            .unwrap();

        assert_eq!(
            wait_run_ids(temp.path()).await,
            None,
            "one unparseable record makes the WHOLE set unsafe — never a partial set that would \
             leave a waited-on run unprotected"
        );
    }
}
