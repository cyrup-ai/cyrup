//! The exclusive store lock, and the identity record that makes a stale or ignored one detectable
//! (ADR-0030 F6 §D).

use cyrup_pico_store::{Storage as _, StorageFailure};

use super::fixture::{self, cx, open_strong};
use crate::{Durability, JsonlOptions, JsonlStore, StoreBusy, StoreLock};

/// The core of the guarantee: one writer per store.
///
/// Taken twice in **one** process on purpose, and it is not a weaker test than two processes would be:
/// `flock(2)` locks the *open file description*, not the process, so a second independent `open` plus
/// `flock` from this process contends exactly as another process's would. `LockFileEx` is per handle for
/// the same reason. The genuinely-two-process case is below, through `flock(1)`.
#[test]
fn a_second_lock_on_one_store_is_refused_and_names_the_holder() {
    let dir = fixture::dir();
    let first = StoreLock::acquire(dir.path()).expect("the first lock");

    let refused = StoreLock::acquire(dir.path()).expect_err("the second lock must be refused");

    match refused {
        StoreBusy::Held { holder, dir: at } => {
            assert_eq!(at, dir.path());
            assert_eq!(
                holder,
                Some(std::process::id()),
                "the refusal must name the holder: the user's question is always 'busy with what?'"
            );
        }
        StoreBusy::Unavailable { source, .. } => {
            panic!("expected a held lock, not an unavailable one: {source}")
        }
    }
    drop(first);
}

/// Releasing is RAII, so the next acquire succeeds with no cleanup step to forget.
#[test]
fn a_released_lock_can_be_retaken() {
    let dir = fixture::dir();
    let first = StoreLock::acquire(dir.path()).expect("the first lock");
    drop(first);
    StoreLock::acquire(dir.path()).expect("the lock after release");
}

/// A genuinely separate process, through `flock(1)`: while this process holds the store, a second one's
/// non-blocking `flock` fails, and once it is released the same command succeeds.
///
/// `flock(1)` rather than a child built from this test binary, because the thing under test is the
/// kernel's lock and not this crate's code: a command that takes `LOCK_EX | LOCK_NB` on the same file
/// is the most direct statement of *"another `cyrup` in this directory is refused"* available.
#[cfg(unix)]
#[test]
fn a_real_second_process_cannot_take_the_lock() {
    use std::process::Command;

    let dir = fixture::dir();
    let lock_path = dir.path().join("store.lock");

    let held = StoreLock::acquire(dir.path()).expect("the lock");
    let refused = Command::new("flock")
        .arg("-n")
        .arg(&lock_path)
        .arg("-c")
        .arg("true")
        .status()
        .expect("running flock(1)");
    assert!(
        !refused.success(),
        "a second process took a lock this process holds"
    );

    drop(held);
    let allowed = Command::new("flock")
        .arg("-n")
        .arg(&lock_path)
        .arg("-c")
        .arg("true")
        .status()
        .expect("running flock(1)");
    assert!(
        allowed.success(),
        "the lock was not released when the guard dropped"
    );
}

/// ADR-0030 F6 §D's signature takes a directory *and* a lock, and nothing in it makes them agree. The
/// mismatch is therefore checked, and this is the check. (The objection — that taking the directory from
/// the lock would make this unrepresentable — is recorded in the crate documentation.)
#[test]
fn a_lock_for_another_directory_cannot_open_this_store() {
    let mine = fixture::dir();
    let other = fixture::dir();
    let lock = StoreLock::acquire(other.path()).expect("a lock on the other directory");

    let refused = JsonlStore::open_for_write(
        mine.path(),
        lock,
        JsonlOptions::new(Durability::ProcessCrash),
    )
    .expect_err("a lock for another directory must not open this store");

    match refused {
        StorageFailure::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput),
        other => panic!("expected an invalid-argument failure, got {other}"),
    }
}

/// A reader takes no lock, which is the half ADR-0030 F6 §D calls *"legal and desirable"*.
#[tokio::test]
async fn a_reader_opens_while_a_writer_holds_the_lock() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (entry, _) = fixture::commit_entry(&mut store, "visible").await;

    let reader = JsonlStore::open_read_only(dir.path()).expect("a read-only open");
    let found = reader.entry(entry).await.expect("the read");

    assert!(
        found.is_some(),
        "a reader must see every commit whose marker is in the log"
    );
    assert_eq!(reader.store_id(), store.store_id());
}

/// And a reader does **not** see a commit whose marker never arrived, which is what makes concurrent
/// reading safe without a lock: the marker is the last line of its commit.
#[tokio::test]
async fn a_reader_does_not_see_an_unconfirmed_tail() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let (entry, _) = fixture::commit_entry(&mut store, "confirmed").await;
    drop(store);

    // A commit interrupted after its record line and before its marker.
    let mut lines = fixture::main_lines(dir.path());
    let unconfirmed = lines
        .iter()
        .find(|l| l.contains("\"t\":\"entry\""))
        .expect("an entry line to copy")
        .replace("\"id\":2", "\"id\":9999");
    lines.push(unconfirmed);
    fixture::write_main_lines(dir.path(), &lines);

    let reader = JsonlStore::open_read_only(dir.path()).expect("a read-only open");
    assert!(
        reader.entry(entry).await.expect("the read").is_some(),
        "the confirmed entry must still be there"
    );
    let unconfirmed_id = reader
        .scan_entries(
            &cyrup_pico_store::EntryQuery::all(cyrup_pico_store::ROOT_CONVERSATION_ID),
            cyrup_pico_store::PageLimit::parse(50).expect("a limit"),
            None,
        )
        .await
        .expect("the scan");
    assert_eq!(
        unconfirmed_id.items.len(),
        1,
        "an entry whose marker never arrived must not be published"
    );
}

/// A reader removes nothing, because a second process truncating a file the writer is appending to is
/// the corruption the lock exists to prevent. *"Ignore"* and *"remove"* are two rules
/// (`spec.md:4429`), and only one of them needs the lock.
#[tokio::test]
async fn a_reader_removes_nothing() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    drop(store);
    fixture::append_raw(dir.path(), b"{\"t\":\"mint\",\"hi");
    let before = std::fs::metadata(crate::wire::main_path(dir.path()))
        .expect("main.jsonl")
        .len();

    let reader = JsonlStore::open_read_only(dir.path()).expect("a read-only open");
    drop(reader);

    let after = std::fs::metadata(crate::wire::main_path(dir.path()))
        .expect("main.jsonl")
        .len();
    assert_eq!(
        before, after,
        "a read-only open must not write to the store"
    );
}

/// The identity record: the writing holder is recorded durably, so a lock that was ignored or left
/// behind by a dead process is detectable (ADR-0030 F6 §D's *"an advisory lock is advisory"*).
#[tokio::test]
async fn the_identity_record_names_the_writing_holder_and_keeps_its_store_id() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;
    let first_id = store.store_id();
    assert_eq!(
        store.reader().identity().holder_pid,
        Some(std::process::id())
    );
    assert_eq!(store.reader().identity().format, crate::FORMAT_VERSION);
    drop(store);

    let again = open_strong(dir.path());
    assert_eq!(
        again.store_id(),
        first_id,
        "the store identity must be durable: a cursor from an earlier open resumes the same scan"
    );
}

/// A format this build does not know is refused rather than read wrongly.
#[test]
fn a_future_format_is_refused() {
    let dir = fixture::dir();
    let store = open_strong(dir.path());
    drop(store);
    let path = dir.path().join("store.json");
    let text = std::fs::read_to_string(&path).expect("store.json");
    let bumped = text.replace(
        &format!("\"format\": {}", crate::FORMAT_VERSION),
        &format!("\"format\": {}", crate::FORMAT_VERSION + 1),
    );
    assert_ne!(text, bumped, "the identity record must carry its format");
    std::fs::write(&path, bumped).expect("rewriting store.json");

    let refused = JsonlStore::open_read_only(dir.path()).expect_err("a future format must refuse");
    match refused {
        StorageFailure::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
        other => panic!("expected an invalid-data failure, got {other}"),
    }
}

/// An undecodable identity record is corruption, not absence: a store that is there and damaged must
/// never read as a store that is not there, because the second answer creates a new one beside it
/// (`spec.md:4352-4360`).
#[test]
fn a_damaged_identity_record_is_corruption_and_not_absence() {
    let dir = fixture::dir();
    drop(open_strong(dir.path()));
    std::fs::write(dir.path().join("store.json"), b"{not json").expect("damaging store.json");

    let refused = JsonlStore::open_read_only(dir.path()).expect_err("damage must refuse");
    assert!(
        matches!(refused, StorageFailure::Corrupt(_)),
        "expected corruption, got {refused}"
    );
}

/// A directory with nothing in it is not a store, and a reader cannot make one.
#[test]
fn a_reader_cannot_create_a_store() {
    let dir = fixture::dir();
    let refused = JsonlStore::open_read_only(dir.path()).expect_err("there is no store here");
    match refused {
        StorageFailure::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected not-found, got {other}"),
    }
    let _ = cx();
}

/// The failure ADR-0030 F6 §D exists to prevent, end to end: a second writer in one directory.
///
/// Through a live [`JsonlStore`] rather than a bare lock, because the composition is the guarantee —
/// the store holds its lock for its whole life, so there is no window between *"the first store is
/// open"* and *"a second `open_for_write` is refused"*, and the refusal arrives before any argument to
/// `open_for_write` can be assembled.
#[tokio::test]
async fn a_second_writer_cannot_open_a_live_store() {
    let dir = fixture::dir();
    let mut store = open_strong(dir.path());
    fixture::seed_root(&mut store).await;

    let refused = StoreLock::acquire(dir.path())
        .expect_err("a second writer must not get a lock on a live store");
    match refused {
        StoreBusy::Held { holder, .. } => assert_eq!(holder, Some(std::process::id())),
        StoreBusy::Unavailable { source, .. } => panic!("expected a held lock: {source}"),
    }

    // And once the first writer is gone, the second one opens and sees everything it committed.
    drop(store);
    let second = open_strong(dir.path());
    assert!(second.reader().last_commit().is_some());
}
