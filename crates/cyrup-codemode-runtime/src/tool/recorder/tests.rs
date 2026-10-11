//! The recorder's cost: how many snapshots go out for `n` changes, what the live list keeps, and
//! that every call's end still reaches a subscriber.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::{ToolCallId, ToolUpdate};

use super::{Gate, MAX_LIVE_ROWS, Recorder};
use crate::tool::{CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeToolDetails};

type Published = Arc<Mutex<Vec<CodemodeToolDetails>>>;

/// A recorder whose snapshots are collected, publishing at most once per `interval`.
fn recorder(interval: Duration) -> (Recorder, Published) {
    let published: Published = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&published);
    let recorder = Recorder::with_publish_interval(
        ToolCallId::from("call-1"),
        Box::new(move |update: ToolUpdate| {
            let details = update.details.expect("a snapshot carries the details");
            sink.lock()
                .unwrap()
                .push(serde_json::from_value(details).unwrap());
        }),
        interval,
    );
    (recorder, published)
}

/// An interval no test outlasts: only the first change is ever admitted on its own.
const NEVER: Duration = Duration::from_secs(3600);

fn running(name: &str) -> CodemodeNestedCall {
    CodemodeNestedCall {
        id: "call-1/?".to_owned(),
        name: name.to_owned(),
        args: String::new(),
        status: CodemodeNestedCallStatus::Running,
        duration_ms: None,
        error: None,
        cost: None,
    }
}

fn end(status: CodemodeNestedCallStatus) -> impl FnOnce(&mut CodemodeNestedCall) {
    move |call| call.status = status
}

#[test]
fn a_snapshot_goes_out_at_most_once_per_interval() {
    let (recorder, published) = recorder(NEVER);
    for i in 0..400 {
        let row = recorder.begin(running(&format!("read{i}")));
        recorder.update(row, end(CodemodeNestedCallStatus::Ok));
    }
    // 800 changes, one snapshot: the first. Before the interval was introduced this was 800.
    assert_eq!(published.lock().unwrap().len(), 1);
}

#[test]
fn every_call_is_reported_once_the_owed_snapshot_is_flushed() {
    let (recorder, published) = recorder(NEVER);
    for i in 0..400 {
        let row = recorder.begin(running(&format!("read{i}")));
        recorder.update(row, end(CodemodeNestedCallStatus::Ok));
    }
    recorder.flush();

    let published = published.lock().unwrap();
    assert_eq!(published.len(), 2, "the first change and the flush");
    let last = published.last().unwrap();
    assert_eq!(last.calls.len(), 400);
    assert!(
        last.calls
            .iter()
            .all(|call| call.status == CodemodeNestedCallStatus::Ok),
        "the end of every call is in the last snapshot"
    );
    assert_eq!(last.calls[399].name, "read399");
    drop(published);

    // Nothing is owed after a flush, so another one publishes nothing.
    recorder.flush();
    assert_eq!(recorder.snapshot().calls.len(), 400);
}

#[test]
fn a_flush_with_nothing_owed_publishes_nothing() {
    let (recorder, published) = recorder(NEVER);
    recorder.flush();
    assert!(published.lock().unwrap().is_empty());
    let row = recorder.begin(running("read"));
    recorder.update(row, end(CodemodeNestedCallStatus::Ok));
    recorder.flush();
    recorder.flush();
    assert_eq!(published.lock().unwrap().len(), 2);
}

#[test]
fn a_zero_interval_publishes_every_change() {
    let (recorder, published) = recorder(Duration::ZERO);
    for i in 0..5 {
        let row = recorder.begin(running(&format!("read{i}")));
        recorder.update(row, end(CodemodeNestedCallStatus::Ok));
    }
    assert_eq!(published.lock().unwrap().len(), 10);
}

#[test]
fn the_gate_admits_again_once_the_interval_has_passed() {
    let started = Instant::now();
    let mut gate = Gate {
        interval: Duration::from_millis(100),
        last: None,
        dirty: false,
    };
    assert!(gate.admit(started), "the first change goes out at once");
    assert!(!gate.admit(started + Duration::from_millis(40)));
    assert!(gate.dirty, "a held-back change is owed");
    assert!(!gate.admit(started + Duration::from_millis(99)));
    assert!(gate.admit(started + Duration::from_millis(100)));
    assert!(!gate.dirty, "publishing settles what was owed");
}

#[tokio::test]
async fn the_publisher_flushes_what_an_interval_held_back() {
    let (recorder, published) = recorder(Duration::from_millis(20));
    let row = recorder.begin(running("read"));
    // Inside the interval of the first publish, so it is owed.
    recorder.update(row, end(CodemodeNestedCallStatus::Ok));
    assert_eq!(published.lock().unwrap().len(), 1);

    let wait = async {
        loop {
            if published.lock().unwrap().len() >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::select! {
        never = recorder.publish_until_dropped() => match never {},
        () = wait => {}
        () = tokio::time::sleep(Duration::from_secs(10)) => panic!("the owed snapshot never went out"),
    }
    let published = published.lock().unwrap();
    assert_eq!(
        published.last().unwrap().calls[0].status,
        CodemodeNestedCallStatus::Ok
    );
}

#[test]
fn the_live_list_keeps_the_latest_rows_and_counts_the_rest_in_one_row() {
    let (recorder, _published) = recorder(NEVER);
    let total = MAX_LIVE_ROWS + 25;
    let rows: Vec<_> = (0..total)
        .map(|i| recorder.begin(running(&format!("read{i}"))))
        .collect();
    // The 25 oldest rows have left the list while still running.
    let snapshot = recorder.snapshot();
    assert_eq!(snapshot.calls.len(), MAX_LIVE_ROWS + 1);
    assert_eq!(snapshot.calls[0].name, "... 25 earlier calls");
    assert_eq!(snapshot.calls[0].id, "call-1/earlier");
    assert_eq!(snapshot.calls[0].status, CodemodeNestedCallStatus::Running);
    assert_eq!(snapshot.calls[1].name, "read25", "the latest rows are kept");
    assert_eq!(
        snapshot.calls[MAX_LIVE_ROWS].name,
        format!("read{}", total - 1)
    );

    // They end after leaving the list: the row follows them, and a row in the list still updates.
    for (i, row) in rows.iter().enumerate().take(25) {
        let status = if i % 5 == 0 {
            CodemodeNestedCallStatus::Error
        } else {
            CodemodeNestedCallStatus::Ok
        };
        recorder.update(*row, end(status));
    }
    recorder.update(rows[total - 1], end(CodemodeNestedCallStatus::Ok));
    let snapshot = recorder.snapshot();
    assert_eq!(snapshot.calls[0].status, CodemodeNestedCallStatus::Error);
    assert_eq!(snapshot.calls[0].args, "5 failed");
    assert_eq!(
        snapshot.calls[MAX_LIVE_ROWS].status,
        CodemodeNestedCallStatus::Ok
    );

    // A script that ends with calls running cancels them, those out of the list too.
    let finished = recorder.finish();
    assert_eq!(finished.len(), MAX_LIVE_ROWS + 1);
    assert!(
        finished[1..MAX_LIVE_ROWS]
            .iter()
            .all(|call| call.status == CodemodeNestedCallStatus::Cancelled)
    );
}

#[test]
fn an_evicted_call_that_is_cancelled_is_counted_as_cancelled() {
    let (recorder, _published) = recorder(NEVER);
    let first = recorder.begin(running("first"));
    for i in 0..MAX_LIVE_ROWS {
        recorder.begin(running(&format!("read{i}")));
    }
    assert_eq!(recorder.snapshot().calls[0].name, "... 1 earlier calls");
    recorder.update(first, end(CodemodeNestedCallStatus::Cancelled));
    let summary = &recorder.snapshot().calls[0];
    assert_eq!(summary.status, CodemodeNestedCallStatus::Cancelled);
    assert_eq!(summary.args, "");
}

#[test]
fn finishing_marks_running_rows_cancelled_and_owes_a_snapshot() {
    // Every earlier change is published, so only the finish itself can owe a snapshot.
    let (recorder, published) = recorder(Duration::ZERO);
    let done = recorder.begin(running("done"));
    recorder.begin(running("cut off"));
    recorder.update(done, end(CodemodeNestedCallStatus::Ok));
    let calls = recorder.finish();
    assert_eq!(calls[1].status, CodemodeNestedCallStatus::Cancelled);
    recorder.flush();
    let published = published.lock().unwrap();
    assert_eq!(
        published.last().unwrap().calls[1].status,
        CodemodeNestedCallStatus::Cancelled,
        "the last snapshot carries the cancellation"
    );
}
