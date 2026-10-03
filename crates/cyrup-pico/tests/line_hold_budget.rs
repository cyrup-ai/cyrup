//! The line-hold budget: ADR-0030 F2's *"guarantee not gained"*, tested as the runtime check it is.
//!
//! §2.1's row for invariant 4 reads *"**unrepresentable** for the reachable path and for nested
//! commit; **guarded** for a captured handle; + **checked** (debug line-hold budget)"*, and F2 says
//! the residue out loud: *"a closure capturing an `Arc<ModelClient>` or a `tokio::process::Command`
//! from its environment still compiles and still awaits inside the held line. No Rust construct
//! forbids that."*
//!
//! This file is the check that catches it, and it is a **separate integration test** for a reason
//! worth stating: the budget and its counters are process-global, so a case that sets a one-millisecond
//! budget would make every other test in the same binary an overrun. One process, one budget.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cyrup_pico::{CommitOutcome, DocToken, SessionMut, hold};
use cyrup_pico_store::{Cx, MemoryStore};

/// The definition this file speaks. `spec.md:1224-1225`: *"ordinary definitions are not registered"*, so
/// a definition is a type and the token is built where it is used.
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

/// A commit that awaits something other than storage is caught; one that does not is not.
#[tokio::test(flavor = "current_thread")]
async fn a_commit_that_awaits_an_external_effect_is_recorded_as_an_overrun() {
    let (_session, handle) = SessionMut::open(Box::new(MemoryStore::new()));
    let cx = Cx::detached();

    // A generous budget first, so the baseline is honest about what a real commit costs.
    hold::set_budget(Duration::from_secs(3600));
    hold::reset();
    let CommitOutcome::NothingToCommit {
        session: handle, ..
    } = handle.commit(&cx, async |tx| Ok(((), tx.writing()))).await
    else {
        panic!("a callback that stages nothing is NothingToCommit");
    };
    assert_eq!(
        hold::overruns(),
        0,
        "an ordinary commit must not trip the budget, or the check catches nothing"
    );

    // Now the hazard: a callback that awaits a timer, standing in for the 60-second model call F2
    // names. Nothing in the type system forbids it, which is why this test exists.
    hold::set_budget(Duration::from_millis(5));
    hold::reset();
    let CommitOutcome::NothingToCommit { .. } = handle
        .commit(&cx, async |tx| {
            tokio::time::sleep(Duration::from_millis(60)).await;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the slow commit still settles: the budget diagnoses, it does not abort");
    };

    assert_eq!(
        hold::overruns(),
        1,
        "a commit that held the line past the budget must be recorded"
    );
    assert!(
        hold::longest() >= Duration::from_millis(55),
        "the recorded hold is the real one: {:?}",
        hold::longest()
    );

    // Restore the shipped default, and prove the zero-duration argument means "default" rather than
    // "every hold is an overrun".
    hold::set_budget(Duration::ZERO);
    assert_eq!(hold::budget(), hold::DEFAULT_BUDGET);

    the_hold_covers_preparation_and_settlement_too().await;
}

/// The budget measures the **whole** window `spec.md:1521-1526` names, not just the callback.
///
/// Deliberately **not** a second `#[test]`: the budget and its counters are process-global, so two
/// cases in one binary would race unless every runner isolated them per process. `cargo nextest` does;
/// `cargo test` does not. One test, so the suite is right under both.
async fn the_hold_covers_preparation_and_settlement_too() {
    let (_session, handle) = SessionMut::open(Box::new(MemoryStore::new()));
    let cx = Cx::detached();
    let address = DocToken::<Live>::define()
        .expect("test.live is a definition")
        .at();

    hold::set_budget(Duration::from_secs(3600));
    hold::reset();
    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(
                &cyrup_pico_doc::Path::new(
                    [cyrup_pico_doc::Seg::key("phase").expect("a safe key")],
                )
                .expect("a safe path"),
                "charging",
            )?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the commit must succeed");
    };

    assert_eq!(hold::overruns(), 0);
    assert!(
        hold::longest() > Duration::ZERO,
        "a real commit took measurable time, so the window was measured at all"
    );
    hold::set_budget(Duration::ZERO);
}
