//! `PageLimit`.

use super::REJECTED_SHAPES;
use crate::{PageLimit, PageLimitError};

#[test]
fn page_limit_rejects_zero() {
    assert_eq!(PageLimit::parse(0), Err(PageLimitError::Zero));
}

#[test]
fn page_limit_rejects_a_value_above_max() {
    assert_eq!(
        PageLimit::parse(PageLimit::MAX + 1),
        Err(PageLimitError::AboveMax {
            requested: PageLimit::MAX + 1,
            max: PageLimit::MAX,
        })
    );
    assert_eq!(
        PageLimit::parse(PageLimit::MAX).map(PageLimit::get),
        Ok(PageLimit::MAX)
    );
}

#[test]
fn page_limit_accepts_one() {
    assert_eq!(PageLimit::parse(1).map(PageLimit::get), Ok(1));
}

#[test]
fn page_limit_rejects_every_shape_that_is_not_an_unsigned_integer() {
    for (shape, json) in REJECTED_SHAPES {
        assert!(
            serde_json::from_str::<PageLimit>(json).is_err(),
            "PageLimit accepted a {shape} ({json})"
        );
    }
}

/// ADR-0030 §10: *"a derived impl over a private `NonZeroU32` accepts values past `MAX`"*. This is
/// the test that says the decode path goes through `parse` and not around it.
#[test]
fn the_decode_path_goes_through_parse_so_max_is_enforced_on_the_way_in() {
    let over = (PageLimit::MAX + 1).to_string();
    assert!(serde_json::from_str::<PageLimit>(&over).is_err());
    let huge = (u64::from(u32::MAX) + 1).to_string();
    assert!(serde_json::from_str::<PageLimit>(&huge).is_err());
    let ok = serde_json::from_str::<PageLimit>("500").expect("500 is a page limit");
    assert_eq!(ok.get(), 500);
    assert_eq!(serde_json::to_string(&ok).expect("limits serialize"), "500");
    assert_eq!(ok.to_string(), "500");
}
