//! `Path`/`Seg`, and the reserved-segment rule.

use crate::{Path, RESERVED_SEGMENTS, Seg};

/// `packages/chord/src/delta/README.md`: *"the tracker never emits `__proto__`, `constructor`, or
/// `prototype` as a path segment"*, and every applier *"reject[s] those segments with
/// `UnsafePathError`"*.
///
/// In Rust those keys carry no prototype hazard at all — they are ordinary `IndexMap` keys. The
/// rejection is kept because it is part of the operation contract a wire batch is validated against:
/// dropping it would mean cyrup applies a batch pi refuses, which is a **behavioural** difference and
/// not a mechanism one.
#[test]
fn a_reserved_segment_cannot_be_put_in_a_path() {
    for reserved in RESERVED_SEGMENTS {
        assert!(
            Seg::key(reserved).is_err(),
            "{reserved} should not be a legal segment"
        );
        assert!(Path::keys([reserved]).is_err());
        assert!(
            Path::new([Seg::Key((*reserved).into())]).is_err(),
            "nor through the variant directly"
        );
    }
}

/// The same rejection on the decode path, because a damaged or hostile record is where an unsafe
/// segment would actually come from.
#[test]
fn a_reserved_segment_fails_the_operation_decode() {
    for reserved in RESERVED_SEGMENTS {
        let json = format!(r#"["s",["{reserved}"],1]"#);
        assert!(
            serde_json::from_str::<crate::Op>(&json).is_err(),
            "{reserved} should not decode as a path segment"
        );
    }
}

#[test]
fn an_ordinary_key_that_merely_looks_suspicious_is_fine() {
    for ok in ["proto", "__proto", "proto__", "Constructor", "prototypes"] {
        assert!(Seg::key(ok).is_ok(), "{ok} is an ordinary key");
    }
}

#[test]
fn the_root_path_is_empty_and_children_extend_it() {
    let root = Path::root();
    assert!(root.is_root());
    assert_eq!(root.segments().len(), 0);

    let child = root
        .child(Seg::key("a").expect("safe"))
        .child(Seg::index(3));
    assert_eq!(child.segments(), &[Seg::Key("a".into()), Seg::Index(3)][..]);
    assert!(!child.is_root());
}

/// A path's wire form is Chord's: a flat array mixing strings and numbers.
#[test]
fn a_path_serializes_as_a_flat_mixed_array() {
    let path = Path::new([Seg::key("items").expect("safe"), Seg::index(2)]).expect("safe");
    assert_eq!(
        serde_json::to_string(&path).expect("serializes"),
        r#"["items",2]"#
    );
}
