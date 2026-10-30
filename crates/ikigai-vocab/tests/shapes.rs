//! Validate the published shapes with the engine used by urn:plan:validate.

use ikigai_vocab::SHAPES;

#[test]
fn a_fork_binding_can_be_cited_by_a_later_step() {
    let data = include_str!("fixtures/plan-fork-binds.ttl");
    let report = ikigai_shacl::validate_outcome(data, SHAPES).expect("validation runs");
    assert!(report.conforms, "{report:#?}");
}
