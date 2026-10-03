//! ADR-0030 §7's first compile-fail case / `G-INV-6`: a `Draft` cannot be stored past the commit
//! callback.
//!
//! Upstream needs **four** mechanisms for this — a revoking `Proxy`, a `#sealed` flag,
//! `#assertOpen()` on every draft access and every `Tx` method, and a validating copy walk
//! (`transaction.ts:663, 673, 898-900`). All four are deleted: `Draft<'d, T>` borrows the
//! transaction, and `'d` cannot outlive the callback body.
//!
//! The failure this prevents is `spec.md:1368-1381`'s own example: a helper returns a draft to a
//! caller that writes to it after the commit, the write is lost, the caller believes the document
//! changed, and the next commit overwrites.
//!
//! Nothing is constructed here on purpose — every value arrives as a parameter, so the error cannot
//! be about building a Session.

use cyrup_pico::{DocAddress, Draft, SessionMut};
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

async fn escape(session: SessionMut, cx: &Cx, address: DocAddress<Live>) {
    let mut escaped: Option<Draft<'_, Empty>> = None;
    let _ = session
        .commit(cx, async |mut tx| {
            let handle = tx.doc(&address, ()).await?;
            escaped = Some(tx.draft(handle)?);
            Ok(((), tx.writing()))
        })
        .await;
    // If this compiled, the next line would write through a draft the Session has already settled.
    if let Some(mut draft) = escaped {
        let _ = draft.delete(&cyrup_pico_doc::Path::root());
    }
}

fn main() {}
