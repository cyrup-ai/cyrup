//! ADR-0030 §7's second compile-fail case / `G-INV-4`, the reachable path: a future that borrows the
//! `Tx` cannot be `tokio::spawn`ed, so the only way to make progress with it is to await it in scope.
//!
//! Upstream needs a pending-operation drain plus a settle check — *"Session commit callback settled
//! before its pending Tx operations"* (`transaction.ts:672-678`). Both become unnecessary.
//!
//! PICO5-PLAN S0 measured which bound fires: with ADR-0030 §10's `_not_send` marker it is `Send`, and
//! without it `'static` rejects the same program. The hazard is closed either way; the marker is kept
//! because F1 puts the commit future inside an actor, where a `!Send` future is the point.

use cyrup_pico::SessionMut;
use cyrup_pico_store::{ConversationId, Cx};

async fn spawned(session: SessionMut, cx: &Cx, conversation: ConversationId) {
    let _ = session
        .commit(cx, async |mut tx| {
            tokio::spawn(async move {
                let _ = tx.conversation(conversation).await;
            });
            Err::<((), _), _>(cyrup_pico::CallbackError::Abandoned("unreachable".into()))
        })
        .await;
}

fn main() {}
