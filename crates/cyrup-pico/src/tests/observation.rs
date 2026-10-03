//! PICO5-PLAN S6's behaviour suite: the four cases the plan names, plus the ones the spec sentences
//! they rest on would otherwise leave unpinned.
//!
//! The plan's four, and where each is:
//!
//! * *"Publication across observers is all-or-nothing, **in a debug build**"* —
//!   [`a_panicking_observer_does_not_truncate_the_publication`], gated on `debug_assertions` because
//!   `[profile.release]`'s `panic = "abort"` (`Cargo.toml:447`) removes the hazard degenerately.
//! * *"Buffered-watch ordering (exact committed frames, in commit order, on activation)"* —
//!   [`buffered_frames_arrive_in_commit_order_with_their_exact_ops`] for the state half and
//!   [`a_watch_delivers_its_buffered_frames_in_order_once_started`] for the watch half.
//! * *"One canary that a published revision has no `&mut` path"* — a compile-fail case,
//!   `tests/compile-fail/a_published_revision_has_no_mutable_path.rs`, because that is the only kind
//!   of test a missing method can fail.
//! * *"A test that `Revision::typed()` failing is one broken document and not a Session failure"* —
//!   [`a_broken_typed_projection_is_one_document_and_not_a_session_failure`].
//!
//! This module is deliberately self-contained — its own definition, its own address — so it does not
//! share fixtures with S4's and S5's suites.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_pico_doc::Op;
use cyrup_pico_store::Cx;
use serde::{Deserialize, Serialize};

use super::store_double::FaultyStore;
use crate::slot::{MAX_PENDING_FRAMES, WatchEnd};
use crate::{
    CommitOutcome, DocDef, DocToken, Frame, FrameContext, Publication, Session, SessionMut,
    SessionScoped, Singleton,
};

/// The value this suite's document holds.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct Live {
    phase: String,
}

/// A value no [`Live`] document can be projected into, for the broken-projection case.
#[derive(Deserialize, Debug)]
struct Counted {
    #[allow(dead_code)]
    count: u64,
}

/// A Session-scoped singleton.
struct LiveDoc;

impl DocDef for LiveDoc {
    type Value = Live;
    type Place = SessionScoped;
    type Shape = Singleton;
    type Seed = ();

    const KIND: &'static str = "test.observe.live";
    const VERSION: u32 = 1;
    const POLICY: () = ();

    fn initial((): ()) -> Live {
        Live {
            phase: "idle".to_owned(),
        }
    }
}

/// A Session with the document already created, plus its token.
async fn opened() -> (Session, SessionMut, DocToken<LiveDoc>, Cx) {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let token = DocToken::<LiveDoc>::define().expect("a valid definition");
    let at = token.at();
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&super::path("phase"), "idle")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    (session, handle, token, cx)
}

/// Set `phase` to `value` in one commit, returning the handle.
async fn set_phase(
    handle: SessionMut,
    cx: &Cx,
    token: &DocToken<LiveDoc>,
    value: &str,
) -> SessionMut {
    let at = token.at();
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(cx, async |mut tx| {
            let doc = tx.doc(&at, ()).await?;
            tx.draft(doc)?.set(&super::path("phase"), value)?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the commit must succeed");
    };
    handle
}

/// `spec.md:1528`: *"`subscribeCommits()` observes complete immutable publications synchronously on
/// the line after adoption."*
#[tokio::test]
async fn a_sync_observer_sees_every_committed_publication() {
    let (session, handle, token, cx) = opened().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let disposer = session.subscribe_commits(move |p: &Publication| {
        // Capture-only: `changes()` hands out `&Change`, whose value is an already-immutable
        // `DocRoot`, so what is retained here is a refcount and not a view into live state.
        let phases: Vec<String> = p
            .changes()
            .iter()
            .filter_map(|c| {
                c.value()
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .collect();
        recorder
            .lock()
            .expect("the recorder is healthy")
            .push(phases);
    });

    let handle = set_phase(handle, &cx, &token, "charging").await;
    let _handle = set_phase(handle, &cx, &token, "draining").await;

    let seen = seen.lock().expect("the recorder is healthy").clone();
    assert_eq!(
        seen,
        vec![vec!["charging".to_owned()], vec!["draining".to_owned()]],
        "both publications reached the observer, in commit order"
    );
    assert_eq!(session.observer_panics(), 0);
    drop(disposer);
}

/// ADR-0030 F5's shell obligation: *"a throwing line observer still truncates the iteration … run them
/// under `catch_unwind`; advance all or none."*
///
/// Upstream's failure is both halves at once — *"a throwing listener truncates the iteration **and**
/// reports a durable, adopted commit as a failure"* — so this asserts both: the observers after the
/// panicking one still ran, and the commit still reported [`CommitOutcome::Committed`].
///
/// **Debug only.** ADR-0030 F1: `panic = "abort"` in release *"means a release build dies instead of
/// partially advancing, which removes pi's partial-advance hazard degenerately rather than solving
/// it"*.
#[cfg(debug_assertions)]
#[tokio::test]
async fn a_panicking_observer_does_not_truncate_the_publication() {
    let (session, handle, token, cx) = opened().await;
    let first = Arc::new(AtomicU32::new(0));
    let third = Arc::new(AtomicU32::new(0));

    let before = Arc::clone(&first);
    let d1 = session.subscribe_commits(move |_: &Publication| {
        before.fetch_add(1, Ordering::SeqCst);
    });
    let d2 = session.subscribe_commits(|_: &Publication| {
        panic!("an observer that misbehaves");
    });
    let after = Arc::clone(&third);
    let d3 = session.subscribe_commits(move |_: &Publication| {
        after.fetch_add(1, Ordering::SeqCst);
    });

    // The panic is deliberate, so its backtrace is not a test failure to report.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let handle = set_phase(handle, &cx, &token, "charging").await;
    std::panic::set_hook(hook);

    assert_eq!(
        first.load(Ordering::SeqCst),
        1,
        "the observer before the panic ran"
    );
    assert_eq!(
        third.load(Ordering::SeqCst),
        1,
        "the observer after the panic ran too: the iteration was not truncated"
    );
    assert_eq!(
        session.observer_panics(),
        1,
        "and the panic was recorded, not swallowed"
    );
    assert_eq!(
        session.publications(),
        2,
        "the durable, adopted commit was still published"
    );
    assert!(handle.is_admitting(), "and the Session is still usable");
    drop((d1, d2, d3));
}

/// `spec.md:1529`: *"Both return idempotent disposers."*
#[tokio::test]
async fn a_disposer_is_idempotent_and_disposes_on_drop() {
    let (session, handle, token, cx) = opened().await;
    let count = Arc::new(AtomicU32::new(0));
    let bump = Arc::clone(&count);
    let disposer = session.subscribe_commits(move |_: &Publication| {
        bump.fetch_add(1, Ordering::SeqCst);
    });

    let handle = set_phase(handle, &cx, &token, "one").await;
    assert_eq!(count.load(Ordering::SeqCst), 1);

    assert!(!disposer.is_disposed());
    disposer.dispose();
    disposer.dispose();
    disposer.dispose();
    assert!(disposer.is_disposed(), "three calls, one effect");

    let handle = set_phase(handle, &cx, &token, "two").await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "a disposed observer sees nothing"
    );

    // And the drop path is the same path.
    let dropped = Arc::new(AtomicU32::new(0));
    let bump = Arc::clone(&dropped);
    {
        let _scoped = session.subscribe_commits(move |_: &Publication| {
            bump.fetch_add(1, Ordering::SeqCst);
        });
    }
    let _handle = set_phase(handle, &cx, &token, "three").await;
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}

/// `spec.md:1530-1532`: *"`subscribeClose()` observes close synchronously when it begins, after
/// admission is sealed."*
#[tokio::test]
async fn a_close_observer_runs_after_admission_is_sealed() {
    let (session, mut handle, _token, cx) = opened().await;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&observed);
    let store = session.store_id();
    let _disposer = session.subscribe_close(move |closing: &crate::Closing| {
        recorder
            .lock()
            .expect("the recorder is healthy")
            .push((closing.store_id(), closing.publications()));
    });

    handle.close(&cx).await.expect("the close must succeed");

    let observed = observed.lock().expect("the recorder is healthy").clone();
    assert_eq!(observed, vec![(store, 1)]);
    assert!(
        !handle.is_admitting(),
        "the seal is taken before the observer runs, so it cannot see an admitting Session"
    );
}

/// PICO5-PLAN S6: *"Buffered-watch ordering (exact committed frames, in commit order, on
/// activation)."*
///
/// `spec.md:3822-3823`: *"publishes every later **exact** committed revision and operation batch
/// without another tracker, value copy, or re-diff."*
#[tokio::test]
async fn buffered_frames_arrive_in_commit_order_with_their_exact_ops() {
    let (session, handle, token, cx) = opened().await;
    let at = token.at();
    let attachment = session
        .attach_doc_state::<Live>(at.address(), &cx)
        .expect("the authority is healthy")
        .expect("the document exists");

    let baseline = attachment.baseline().expect("a baseline");
    assert_eq!(
        baseline.typed().expect("the baseline fits").phase,
        "idle",
        "the acquisition revision is the exact committed one"
    );

    let handle = set_phase(handle, &cx, &token, "charging").await;
    let handle = set_phase(handle, &cx, &token, "draining").await;
    let _handle = set_phase(handle, &cx, &token, "idle").await;
    assert_eq!(
        attachment.buffered(),
        3,
        "three commits buffered while unactivated"
    );
    assert_eq!(
        attachment
            .baseline()
            .expect("a baseline")
            .typed()
            .expect("fits")
            .phase,
        "idle",
        "and the baseline did not move: nothing was delivered yet"
    );

    let state = attachment.activate();
    let mut phases = Vec::new();
    while let Some(frame) = state.try_next_frame() {
        let Frame::Revision { value, ops, .. } = frame else {
            panic!("no retirement happened");
        };
        assert_eq!(ops.len(), 1, "one commit, one operation");
        assert!(
            matches!(ops.ops().first(), Some(Op::Set { .. })),
            "the exact operation batch, not a re-diff"
        );
        phases.push(value.typed().expect("the value fits").phase);
    }
    assert_eq!(
        phases,
        vec!["charging", "draining", "idle"],
        "in commit order"
    );
    assert_eq!(state.buffered(), 0);
}

/// `spec.md:1515-1517`: *"a later commit already present becomes the baseline, while one committed
/// after registration is delivered."*
#[tokio::test]
async fn a_commit_already_present_becomes_the_baseline() {
    let (session, handle, token, cx) = opened().await;
    let at = token.at();
    let handle = set_phase(handle, &cx, &token, "charging").await;

    let attachment = session
        .attach_doc_state::<Live>(at.address(), &cx)
        .expect("the authority is healthy")
        .expect("the document exists");
    assert_eq!(
        attachment
            .baseline()
            .expect("a baseline")
            .typed()
            .expect("fits")
            .phase,
        "charging",
        "the commit already present is the baseline"
    );
    assert_eq!(
        attachment.buffered(),
        0,
        "and it was not also delivered as a frame"
    );

    let _handle = set_phase(handle, &cx, &token, "draining").await;
    assert_eq!(attachment.buffered(), 1, "the next commit is delivered");
}

/// PICO5-PLAN S6: *"A test that `Revision::typed()` failing is one broken document and not a Session
/// failure."*
#[tokio::test]
async fn a_broken_typed_projection_is_one_document_and_not_a_session_failure() {
    let (session, handle, token, cx) = opened().await;
    let at = token.at();
    let attachment = session
        .attach_doc_state::<Counted>(at.address(), &cx)
        .expect("the authority is healthy")
        .expect("the document exists");

    let baseline = attachment.baseline().expect("a baseline");
    let failure = baseline
        .typed()
        .expect_err("a `Live` document does not fit `Counted`");
    assert_eq!(
        failure.document,
        session
            .current_incarnation(at.address())
            .expect("healthy")
            .expect("one incarnation"),
        "the failure names the one broken document"
    );
    assert!(
        baseline.raw().get("phase").is_some(),
        "and the raw revision is still readable: the infallible half never fails"
    );

    // Another consumer of the same revision, with the right type, is unaffected.
    let honest = session
        .attach_doc_state::<Live>(at.address(), &cx)
        .expect("healthy")
        .expect("exists");
    assert_eq!(
        honest
            .baseline()
            .expect("a baseline")
            .typed()
            .expect("fits")
            .phase,
        "idle"
    );

    // And the Session still commits.
    let handle = set_phase(handle, &cx, &token, "charging").await;
    assert!(handle.is_admitting());
    assert_eq!(session.publications(), 2);
}

/// `spec.md:3867-3872`: *"`start()` installs the sole listener and schedules delivery; it never invokes
/// user code inline. … Immediately before invocation, `watch.value` advances to that value."*
#[tokio::test]
async fn a_watch_delivers_its_buffered_frames_in_order_once_started() {
    let (session, handle, token, cx) = opened().await;
    let at = token.at();
    let watch = session
        .watch_doc::<Live>(at.address(), &cx)
        .expect("healthy")
        .expect("exists");

    let handle = set_phase(handle, &cx, &token, "charging").await;
    let _handle = set_phase(handle, &cx, &token, "draining").await;
    assert_eq!(watch.buffered(), 2);
    assert_eq!(
        watch.value().expect("a value").typed().expect("fits").phase,
        "idle",
        "before `start`, the value is still the acquisition revision"
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let (watching, delivery) = watch.start(move |frame: Frame<Live>| {
        let recorder = Arc::clone(&recorder);
        async move {
            if let Some(value) = frame.value() {
                recorder
                    .lock()
                    .expect("the recorder is healthy")
                    .push(value.typed().expect("fits").phase);
            }
            Ok(())
        }
    });
    let driver = tokio::spawn(delivery);

    // Drive the loop until both frames landed.
    for _ in 0..1_000 {
        if seen.lock().expect("healthy").len() == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        seen.lock().expect("healthy").clone(),
        vec!["charging".to_owned(), "draining".to_owned()]
    );
    assert_eq!(
        watching
            .value()
            .expect("a value")
            .typed()
            .expect("fits")
            .phase,
        "draining",
        "the handle's value advanced with delivery"
    );

    // `stop()` is idempotent and both calls report the same terminal reason.
    assert!(matches!(watching.stop(), WatchEnd::Stopped));
    assert!(matches!(watching.stop(), WatchEnd::Stopped));
    assert!(matches!(
        driver.await.expect("the delivery task joins"),
        WatchEnd::Stopped
    ));
}

/// `spec.md:3896-3898`: *"Retirement queues the exact terminal `[["r", null]]` frame … After that
/// callback settles, the watch closes as `retired` and never follows a replacement incarnation."*
#[tokio::test]
async fn a_retirement_is_terminal_and_closes_the_subscription() {
    let (session, handle, token, cx) = opened().await;
    let at = token.at();
    let state = session
        .attach_doc_state::<Live>(at.address(), &cx)
        .expect("healthy")
        .expect("exists")
        .activate();

    let retire_at = token.at();
    let CommitOutcome::Committed {
        session: _handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            tx.retire_doc(&retire_at).await?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the retiring commit must succeed");
    };

    let frame = state.try_next_frame().expect("the terminal frame");
    assert!(frame.is_terminal(), "retirement is the terminal frame");
    assert!(
        frame.value().is_none(),
        "and carries no value: never stale state"
    );
    assert!(matches!(state.end(), Some(WatchEnd::Retired)));
    assert!(
        state.value().is_none(),
        "a consumer that stays exposed sees absence, not the last value"
    );
    assert!(
        state.try_next_frame().is_none(),
        "and nothing follows a retirement"
    );
}

/// `spec.md:3875-3879`: *"Adding frame 101 replaces the complete undelivered suffix with one
/// self-contained root replacement `[["r", newestValue]]`, using the newest exact immutable
/// revision."*
#[tokio::test]
async fn overflow_collapses_the_undelivered_suffix_into_one_root_replacement() {
    let (session, handle, token, cx) = opened().await;
    let at = token.at();
    let state = session
        .attach_doc_state::<Live>(at.address(), &cx)
        .expect("healthy")
        .expect("exists")
        .activate();

    let mut handle = Some(handle);
    for i in 0..=MAX_PENDING_FRAMES {
        let owned = handle.take().expect("the handle comes back");
        handle = Some(set_phase(owned, &cx, &token, &format!("step-{i}")).await);
    }
    assert_eq!(
        state.buffered(),
        1,
        "the suffix collapsed into exactly one frame"
    );

    let Some(Frame::Revision { value, ops, .. }) = state.try_next_frame() else {
        panic!("one collapsed revision frame");
    };
    assert_eq!(ops.len(), 1);
    let Some(Op::ReplaceRoot(root)) = ops.ops().first() else {
        panic!("the replacement is a root replacement");
    };
    let newest = format!("step-{MAX_PENDING_FRAMES}");
    assert_eq!(
        root.get("phase").and_then(|v| v.as_str()),
        Some(newest.as_str()),
        "with the newest exact immutable revision"
    );
    assert_eq!(value.typed().expect("fits").phase, newest);
    assert!(state.try_next_frame().is_none());
}

/// `spec.md:1531`: *"the Harness stops watches … there"* — close ends every subscription.
#[tokio::test]
async fn closing_the_session_ends_every_subscription() {
    let (session, mut handle, token, cx) = opened().await;
    let at = token.at();
    let state = session
        .attach_doc_state::<Live>(at.address(), &cx)
        .expect("healthy")
        .expect("exists")
        .activate();
    assert!(state.end().is_none());

    handle.close(&cx).await.expect("the close must succeed");

    assert!(matches!(state.end(), Some(WatchEnd::SessionClosed)));
    assert_eq!(
        handle.session().subscription_documents(),
        0,
        "and the registry was drained, not left holding dead entries"
    );
    assert!(
        state.next_frame().await.is_none(),
        "a consumer waiting on a closed Session is told, not left waiting"
    );
}

/// ADR-0030 open question 6, as far as a runtime test can see it: the values are carried, and there is
/// nothing on this type that cancels. The absence is pinned by
/// `tests/compile-fail/a_frame_context_cannot_inherit_the_producers_cancellation.rs`.
#[test]
fn a_frame_context_carries_values_and_nothing_that_cancels() {
    let empty = FrameContext::empty();
    assert!(empty.is_empty());
    assert!(empty.get("trace").is_none());

    let mut values = cyrup_pico_doc::DocMap::new();
    values.insert("trace".into(), cyrup_pico_doc::DocValue::string("abc123"));
    let carried = FrameContext::from_values(values);
    assert_eq!(carried.len(), 1);
    assert_eq!(
        carried
            .get("trace")
            .and_then(cyrup_pico_doc::DocValue::as_str),
        Some("abc123")
    );
}
