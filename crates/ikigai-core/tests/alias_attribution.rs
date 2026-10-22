//! A hop's rule indices belong to the table revision that produced it.
use ikigai_core::{AliasHop, AliasTable, Iri};

fn hop(table: &AliasTable, target: &str) -> AliasHop {
    table
        .canonicalize(&Iri::parse(target).unwrap())
        .hop()
        .unwrap()
        .clone()
}

#[test]
fn a_foreign_hop_does_not_panic_or_attribute_a_miss() {
    let origin = AliasTable::new().exact("urn:example:old", "urn:example:new");
    let rewritten = hop(&origin, "urn:example:old");
    AliasTable::new().record_unresolved(&rewritten);
    let other = AliasTable::new().exact("urn:example:other", "urn:example:else");
    other.record_unresolved(&rewritten);
    assert_eq!(other.rules()[0].unresolved(), 0);
    origin.record_unresolved(&rewritten);
    assert_eq!(origin.rules()[0].unresolved(), 1);
    assert_eq!(origin.rules()[0].hops(), 1);
    assert_eq!(origin.rules()[0].refused(), 0);
}

#[test]
fn a_hop_from_before_a_resort_does_not_attribute_to_a_different_rule() {
    let table = AliasTable::new().exact("urn:example:a", "urn:example:b");
    let rewritten = hop(&table, "urn:example:a");
    let table = table.exact("urn:example:longer", "urn:example:destination");
    table.record_unresolved(&rewritten);
    assert!(table.rules().iter().all(|rule| rule.unresolved() == 0));
    table.record_unresolved(&hop(&table, "urn:example:a"));
    assert_eq!(table.rules()[0].unresolved(), 0);
    assert_eq!(table.rules()[1].unresolved(), 1);
}

#[test]
fn validating_a_stale_hop_does_not_record_a_new_refusal() {
    let table = AliasTable::new().exact("urn:example:a", "urn:example:b");
    let rewritten = hop(&table, "urn:example:a");
    let table = table.exact("urn:example:b", "urn:example:a");
    table.record_unresolved(&rewritten);
    assert!(table.rules().iter().all(|rule| rule.unresolved() == 0));
    assert!(table.rules().iter().all(|rule| rule.refused() == 0));
    assert_eq!(table.rules().iter().map(|rule| rule.hops()).sum::<u64>(), 1);
}
