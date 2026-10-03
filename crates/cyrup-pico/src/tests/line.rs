//! The actor and the ticket: ADR-0030 F1's *"`SessionMut` lives in exactly one place"*.

use cyrup_pico_store::Cx;
use tokio::task::LocalSet;

use super::store_double::{Fault, FaultyStore};
use super::{live_address, path};
use crate::{CommitOutcome, CommitReply, Committer, Line, SessionMut};

/// A ticket commits through the line, and the line hands the Session back on a clean shutdown.
#[tokio::test]
async fn a_ticket_commits_through_the_line() {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let (committer, line) = Line::new(handle, 4);
    let local = LocalSet::new();
    let driving = local.spawn_local(line.run());

    let address = live_address();
    let reply = local
        .run_until(committer.commit(Cx::detached(), async move |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        }))
        .await
        .expect("the line is alive");
    assert!(matches!(reply, CommitReply::Committed { .. }));
    assert_eq!(session.publications(), 1);

    drop(committer);
    local.await;
    let back = driving.await.expect("the line task did not panic");
    assert!(
        back.is_some(),
        "a clean shutdown hands the mutation handle back for `close()`"
    );
}

/// The first fatal path kills the loop and closes **every** ticket.
///
/// The one whose commit was fatal is told directly, through [`CommitReply::SessionDead`]; every other
/// ticket holder learns from the closed channel, which is exactly the weaker, `checked` guarantee
/// ADR-0030 F1 says the [`Committer`] boundary has — and which it says out loud rather than claiming
/// the consuming form while shipping an `Arc`.
#[tokio::test]
async fn an_uncertain_commit_kills_the_loop_and_closes_every_ticket() {
    let (store, switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let (committer, line) = Line::new(handle, 4);
    // A second ticket, held by somebody else entirely.
    let bystander: Committer = committer.clone();
    let local = LocalSet::new();
    let driving = local.spawn_local(line.run());

    switch.arm(Fault::Uncertain);
    let address = live_address();
    let reply = local
        .run_until(committer.commit(Cx::detached(), async move |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        }))
        .await
        .expect("the reply arrives before the loop returns");
    assert!(
        matches!(reply, CommitReply::SessionDead(_)),
        "the committing ticket is told directly"
    );

    drop(committer);
    local.await;
    let back = driving.await.expect("the line task did not panic");
    assert!(back.is_none(), "there is no handle to hand back");
    assert!(bystander.is_closed(), "every later ticket call fails");

    let second: Result<CommitReply<()>, _> = bystander
        .commit(Cx::detached(), async move |tx| Ok(((), tx.writing())))
        .await;
    assert!(
        second.is_err(),
        "a bystander learns the Session is dead from a closed channel"
    );
    assert_eq!(session.publications(), 0);
}

/// The **adoption** path does the same thing at the actor, which is the half a port skips.
#[tokio::test]
async fn an_adoption_failure_also_kills_the_loop() {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    // Create the document outside the line, so the fatal commit is an adoption into an existing
    // tracker.
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    let incarnation = session
        .current_incarnation(address.address())
        .expect("the authority is healthy")
        .expect("the document was created");

    let (committer, line) = Line::new(handle, 2);
    let local = LocalSet::new();
    let driving = local.spawn_local(line.run());

    crate::fault::set_before_adopt(move |window| window.forget(incarnation));
    let addr = address.clone();
    let reply = local
        .run_until(committer.commit(Cx::detached(), async move |mut tx| {
            let doc = tx.doc(&addr, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        }))
        .await
        .expect("the reply arrives");
    crate::fault::clear();
    assert!(matches!(reply, CommitReply::SessionDead(_)));

    drop(committer);
    local.await;
    assert!(
        driving.await.expect("no panic").is_none(),
        "the loop returns with no handle"
    );
    assert_eq!(session.publications(), 1, "only the first commit published");
}
