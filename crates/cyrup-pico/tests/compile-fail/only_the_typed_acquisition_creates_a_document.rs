//! `G-ONLY-TYPED-ACQUISITION-CREATES` / `spec.md:1219`: *"only public typed `tx.doc()` is
//! get-or-create. Internal fork copying may create new incarnations directly from stored values
//! without a definition."*
//!
//! PICO5-PLAN S4 shipped a **public** untyped `Tx::doc(&DocumentAddress, DocSeed)` as the seam this
//! slice was to build on, and with it the rule above was a convention: any caller could mint an
//! incarnation with a kind, a scope policy and an initial value assembled from four unrelated
//! arguments. S5 makes `Tx::doc_at` and its `Seed` crate-private, so the only creating path an
//! application can name is one that goes through a `DocToken` — which is what makes the kind, the
//! version, the scope policy, the initial value and the migration all come from **one definition**.
//!
//! The error to expect is `E0624` (a private associated function) rather than `E0599`: the method
//! exists, and that is the point — it is reachable from inside the kernel, where fork copying will
//! need it (PICO5-PLAN S9), and from nowhere else.

use cyrup_pico::SessionMut;
use cyrup_pico_store::{Cx, DocumentAddress};

async fn forge(session: SessionMut, cx: &Cx, address: DocumentAddress) {
    let _ = session
        .commit::<(), _>(cx, async |mut tx| {
            let _ = tx.doc_at(&address, ());
            Ok(((), tx.writing()))
        })
        .await;
}

fn main() {}
