//! `G-FORK-POLICY-PERSISTED`, from the signature's side.
//!
//! `spec.md:1478-1479` persists the policy per incarnation, and `spec.md:1491-1494`'s rejection exists
//! so the fork has *"one unambiguous stored source revision"*. Both sentences are about the fork not
//! choosing: it reads what each of the parent's records says. So
//! [`cyrup_pico::Tx::fork_conversation`] takes a [`ConversationOwnership`] and a
//! [`ConversationParent`] and **nothing else** — no policy, no
//! [`DocToken`](cyrup_pico::DocToken), no [`DocAddress`](cyrup_pico::DocAddress) — and a caller that
//! tries to override one argument-passes nothing that is accepted.
//!
//! ADR-0030 §7's rule is why this is a file and not a comment: a guarantee carried by a signature is
//! invisible in a diff when the signature changes, and no runtime test turns red. `E0061`.
//!
//! [`ConversationOwnership`]: cyrup_pico::ConversationOwnership
//! [`ConversationParent`]: cyrup_pico_store::ConversationParent

use cyrup_pico::{ConversationOwnership, SessionMut};
use cyrup_pico_store::{ConversationParent, Cx, RewindableFork};

async fn forge(session: SessionMut, cx: &Cx, parent: ConversationParent) {
    let _ = session
        .commit::<(), _>(cx, async |tx| {
            let mut tx = tx.writing();
            let _ = tx
                .fork_conversation(
                    ConversationOwnership::Ownerless,
                    parent,
                    RewindableFork::Current,
                )
                .await;
            Ok(((), tx))
        })
        .await;
}

fn main() {}
