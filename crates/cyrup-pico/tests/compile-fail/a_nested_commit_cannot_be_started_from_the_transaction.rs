//! `G-INV-4`, the nested-commit half: there is no method on the transaction through which a nested
//! commit can be started.
//!
//! This is the hazard upstream *does not even diagnose*. A hook helper `record_usage(session, ..)`
//! calls `session.commit(..)`, somebody calls it from inside a commit callback, and the nested call
//! queues behind the current job on `#tail` and deadlocks the Session **permanently with no
//! diagnostic**.
//!
//! PICO5-PLAN S4 asks for a canary whose content is *"that the type has no such method"*. A
//! `trybuild` case pins exactly that, where a comment would not: `E0599` is what a later change
//! adding a convenience `Tx::commit` would turn green, and the diff would otherwise look harmless.
//!
//! Both halves of the absence are named: no `commit`, and no accessor that hands back a `Session` to
//! commit *through*.

use cyrup_pico::SessionMut;
use cyrup_pico_store::Cx;

async fn nested(session: SessionMut, cx: &Cx) {
    let _ = session
        .commit(cx, async |tx| {
            let _ = tx.commit(cx, async |inner| Ok(((), inner.writing()))).await;
            let _ = tx.session();
            Ok(((), tx.writing()))
        })
        .await;
}

fn main() {}
