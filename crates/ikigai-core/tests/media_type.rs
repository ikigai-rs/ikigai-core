//! The media-type comparison is PUBLIC (ledger #2877): a module that asks "is this JSON?" or
//! "is this the type I declared?" calls core's one implementation of RFC 9110 §8.3.1 instead of
//! stripping parameters by hand. These pin it from OUTSIDE the crate, at the shapes the
//! hand-written copies in ikigai-json (`is_json`) and ikigai-engine (`bare`) answer today.

use ikigai_core::{media_type_essence, media_types_agree};

#[test]
fn the_essence_drops_parameters_whitespace_and_case() {
    assert_eq!(media_type_essence("application/json"), "application/json");
    assert_eq!(
        media_type_essence("Application/JSON; charset=UTF-8"),
        "application/json"
    );
    assert_eq!(media_type_essence("  text/turtle ;q=1"), "text/turtle");
    assert_eq!(
        media_type_essence("application/ld+json;profile=\"x\""),
        "application/ld+json"
    );
    // No validation: a string with no `/` is returned trimmed and lowercased, never refused.
    assert_eq!(media_type_essence(" Turtle "), "turtle");
    assert_eq!(media_type_essence(""), "");
}

#[test]
fn agreement_is_the_essence_plus_any_parameter_both_state() {
    assert!(media_types_agree("text/csv", "text/csv; charset=utf-8"));
    assert!(media_types_agree("text/csv; charset=utf-8", "text/csv"));
    assert!(media_types_agree(
        "Text/CSV;charset=\"UTF-8\"",
        "text/csv; charset=utf-8"
    ));
    assert!(!media_types_agree(
        "text/csv; charset=utf-8",
        "text/csv; charset=iso-8859-1"
    ));
    assert!(!media_types_agree("text/csv", "text/tab-separated-values"));
    // Only `charset` compares case-insensitively; another parameter's value is exact.
    assert!(!media_types_agree(
        "application/ld+json; profile=A",
        "application/ld+json; profile=a"
    ));
}
