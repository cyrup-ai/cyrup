//! `classify_version` — `spec.md:1441-1448`'s four-way comparison, once.

use crate::tests::{base, root, ver};
use crate::{DocValue, ReplayPlan, StoredVersion, VersionFit, classify_version};

/// A witness for a stored version, minted the only way it can be: from a parsed record set.
fn stored(v: u32) -> StoredVersion {
    ReplayPlan::parse(&[base(v, root(&[("a", DocValue::integer(1))]))])
        .expect("replayable")
        .version()
}

/// The whole table, including both reject arms — which are different rejections and would be easy to
/// collapse into one.
#[test]
fn the_four_way_comparison_is_the_spec_s() {
    assert_eq!(
        classify_version(stored(2), ver(2), true),
        VersionFit::Current,
        "stored == token -> use value"
    );
    assert_eq!(
        classify_version(stored(2), ver(2), false),
        VersionFit::Current,
        "an equal version needs no migration callback"
    );
    assert_eq!(
        classify_version(stored(1), ver(3), true),
        VersionFit::Migrate {
            from: ver(1),
            to: ver(3)
        },
        "stored < token -> call migrate(value, storedVersion)"
    );
    assert_eq!(
        classify_version(stored(1), ver(3), false),
        VersionFit::NoMigration {
            stored: ver(1),
            token: ver(3)
        },
        "no migrate -> reject older stored version"
    );
    assert_eq!(
        classify_version(stored(4), ver(3), true),
        VersionFit::StoredIsNewer {
            stored: ver(4),
            token: ver(3)
        },
        "stored > token -> reject typed access, migration callback or not"
    );
    assert_eq!(
        classify_version(stored(4), ver(3), false),
        VersionFit::StoredIsNewer {
            stored: ver(4),
            token: ver(3)
        }
    );
}

/// The witness carries the version the **newest base** was written at, which is the version the tail
/// must agree with — so a delta cannot claim a version nobody read.
#[test]
fn the_witness_names_the_version_of_the_selected_base() {
    let plan = ReplayPlan::parse(&[
        base(1, root(&[("a", DocValue::integer(1))])),
        base(5, root(&[("a", DocValue::integer(2))])),
    ])
    .expect("replayable");
    assert_eq!(plan.version().version(), ver(5));
}

/// `DefVersion` orders, and `FIRST` is 1 — the two facts migration comparisons rest on. (Its parser
/// cases live with S1's suite in `cyrup-pico-store`, which re-exports this type.)
#[test]
fn versions_order_and_the_first_is_one() {
    assert_eq!(crate::DefVersion::FIRST, ver(1));
    assert!(ver(1) < ver(2));
    assert_eq!(ver(7).get(), 7);
    assert_eq!(ver(7).to_string(), "7");
}
