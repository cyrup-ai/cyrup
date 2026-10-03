//! ADR-0030 F2's branded handle: a `DocHandle<'tx, T>` minted in one commit cannot be used in
//! another — *"a hazard pi has no concept of"*, because upstream's handle is an ordinary object.
//!
//! `'tx` is generative: `commit` chooses it, the caller cannot name it, and
//! `PhantomData<&'tx mut ()>` makes the brand **invariant** so it cannot be widened to a lifetime two
//! calls share. The rejection therefore lands where the handle tries to *leave* the first transaction,
//! which is the earliest point it could, and a stale handle resolving to the wrong slot index in a
//! later transaction has no spelling.
//!
//! PICO5-PLAN S0 singled this out as the one negative case whose diagnostic is poor — a bare
//! `lifetime may not live long enough` that names neither handles nor transactions. The committed
//! stderr beside this file is therefore load-bearing twice over: it pins the rejection, and it is the
//! message `DocHandle`'s own documentation quotes and supplies the fix for.

use cyrup_pico::{DocAddress, SessionMut, Settled};
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

async fn crossed(first: SessionMut, second: SessionMut, cx: &Cx, address: DocAddress<Live>) {
    let out = first
        .commit(cx, async |mut tx| {
            let handle = tx.doc(&address, ()).await?;
            Ok((handle, tx.writing()))
        })
        .await;
    let Ok((Settled::Committed { result: handle, .. }, _)) = out.or_fatal() else {
        return;
    };
    let _ = second
        .commit(cx, async |mut tx| {
            let _ = tx.draft(handle)?;
            Ok(((), tx.writing()))
        })
        .await;
}

fn main() {}
