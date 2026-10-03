//! ADR-0030 §2.1 invariant 6 / `G-INV-6`, the **return** half: a `Draft` cannot be the commit
//! callback's result value.
//!
//! This is `spec.md:1368-1381`'s own documented footgun: a harness helper does
//! `let d = tx.doc(..).await?` and hands `d` back to its caller, which writes to it after the commit
//! settled. The write is lost, the caller believes the document changed, and the next commit
//! overwrites. Upstream needs a revoking `Proxy`, a `#sealed` flag and `#assertOpen()` on every
//! access to turn that into a thrown error at the write; here the value cannot leave the callback.
//!
//! `a_draft_cannot_outlive_the_commit_callback.rs` pins the *escape-to-an-outer-binding* half, which
//! is a different program with a different error. PICO5-PLAN S0 names the three separately on purpose:
//! escape, return and spawn are each how a well-meaning change would be written, and each is rejected
//! at a different point.

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

async fn returned(session: SessionMut, cx: &Cx, address: DocAddress<Live>) {
    let _ = session
        .commit(cx, async |mut tx| {
            let handle = tx.doc(&address, ()).await?;
            let draft = tx.draft(handle)?;
            Ok((draft, tx.writing()))
        })
        .await;
}

fn main() {}
