//! ADR-0030 §7's third compile-fail case / `G-READ-BEFORE-FIRST-TABLE-WRITE`.
//!
//! Within one commit, every table row the transaction needs is read **before** its first table write
//! (`spec.md:1553-1557`). Upstream enforces that with a monotone `#hasTableWrite` boolean and a thrown
//! `ReadAfterWrite` (`errors.ts:4-9`), mid-transaction, from deep inside harness logic — the whole
//! commit rolls back, the turn fails, and the user watches the assistant's message vanish.
//!
//! §7 says why this one is *"worth a `trybuild` case"* specifically: it is the one guarantee here that
//! replaces a named upstream error class with nothing but an **absent method**, so its deletion is
//! invisible in a diff and no runtime test turns red.
//!
//! The message is part of the deliverable. PICO5-PLAN S0 requires a *method-not-found* error rather
//! than a lifetime error, and all four table readers are named so the committed stderr shows the
//! absence is uniform rather than a quirk of one.

use cyrup_pico::SessionMut;
use cyrup_pico_store::{ConversationId, Cx, EntryId, SubmissionId, TaskId};

async fn read_after_write(
    session: SessionMut,
    cx: &Cx,
    conversation: ConversationId,
    entry: EntryId,
    task: TaskId,
    submission: SubmissionId,
) {
    let _ = session
        .commit(cx, async |tx| {
            // The natural, wrong order: write first, then read the record the next statement
            // branches on. ADR-0030 F2 calls this out as the realistic failure.
            let mut tx = tx.writing();
            let a = tx.conversation(conversation).await?;
            let b = tx.entry(entry).await?;
            let c = tx.task(task).await?;
            let d = tx.submission(submission).await?;
            Ok(((a, b, c, d), tx))
        })
        .await;
}

fn main() {}
