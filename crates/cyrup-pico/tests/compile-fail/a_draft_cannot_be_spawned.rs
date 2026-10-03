//! ADR-0030 §2.1 invariant 6 / `G-INV-6`, the **spawn** half: a `Draft` cannot be moved into a
//! spawned task.
//!
//! PICO5-PLAN S0 names this case separately from the escape and the return because a spawn is how a
//! well-meaning *"do the document write in the background"* change gets written, and it is the one
//! shape where the author believes the draft is still live.
//!
//! Two bounds close it and the committed stderr records which one fires: `Draft<'d, T>` borrows the
//! transaction, so `'d` is not `'static`, and it reaches through that borrow to a `Tx`, so it is
//! `!Send`. `Send` is reported first. Either bound alone is enough, which is the same finding
//! `a_future_borrowing_the_transaction_cannot_be_spawned.rs` records for the `Tx` itself — so a change
//! that makes `Tx` `Send` for F1's actor does not reopen this.

use cyrup_pico::{DocAddress, SessionMut};
use cyrup_pico_store::Cx;

struct Live;

#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Live {
    type Value = Empty;
    type Place = cyrup_pico::SessionScoped;
    type Shape = cyrup_pico::Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

async fn spawned(session: SessionMut, cx: &Cx, address: DocAddress<Live>) {
    let _ = session
        .commit(cx, async |mut tx| {
            let handle = tx.doc(&address, ()).await?;
            let mut draft = tx.draft(handle)?;
            tokio::spawn(async move {
                let _ = draft.delete(&cyrup_pico_doc::Path::root());
            });
            Ok(((), tx.writing()))
        })
        .await;
}

fn main() {}
