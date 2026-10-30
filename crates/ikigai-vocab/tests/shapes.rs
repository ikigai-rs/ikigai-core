//! Validate the published shapes with the engine used by urn:plan:validate.

use ikigai_vocab::SHAPES;

const FORK_BINDS: &str = include_str!("fixtures/plan-fork-binds.ttl");

fn assert_conforms(name: &str, data: &str) {
    let report = ikigai_shacl::validate_report(data, SHAPES).expect("validation runs");
    assert!(report.conforms, "{name}: {report:#?}");
}

fn assert_unbound(data: &str) {
    let report = ikigai_shacl::validate_report(data, SHAPES).expect("validation runs");
    assert!(!report.conforms, "an unbound reference must fail");
    assert_eq!(report.violations.len(), 1, "{report:#?}");
    let violation = &report.violations[0];
    assert_eq!(violation.focus_node, "urn:plan:fork-binds");
    assert_eq!(
        violation.component,
        "http://www.w3.org/ns/shacl#SPARQLConstraintComponent"
    );
    assert_eq!(
        violation.value,
        Some(ikigai_shacl::Term::Uri {
            value: "urn:plan:fork-binds:var:report".into()
        })
    );
    assert!(
        violation
            .message
            .as_ref()
            .unwrap()
            .lexical()
            .contains("references a name this plan never binds"),
        "{violation:#?}"
    );
}

#[test]
fn a_fork_binding_can_be_cited_by_a_later_step() {
    assert_conforms("plan-fork-binds", FORK_BINDS);
}

#[test]
fn all_plan_fixtures_satisfy_the_published_shapes() {
    let fixtures = [
        (
            "plan-linkcheck",
            include_str!("fixtures/plan-linkcheck.ttl"),
        ),
        (
            "plan-linkcheck-strict",
            include_str!("fixtures/plan-linkcheck-strict.ttl"),
        ),
        (
            "plan-linkcheck-repair",
            include_str!("fixtures/plan-linkcheck-repair.ttl"),
        ),
        (
            "plan-linkcheck-scoped",
            include_str!("fixtures/plan-linkcheck-scoped.ttl"),
        ),
        (
            "plan-fork-branch",
            include_str!("fixtures/plan-fork-branch.ttl"),
        ),
        (
            "plan-fork-mapped",
            include_str!("fixtures/plan-fork-mapped.ttl"),
        ),
    ];
    for (name, data) in fixtures {
        assert_conforms(name, data);
    }
}

#[test]
fn a_missing_fork_binding_is_still_refused() {
    assert_unbound(&FORK_BINDS.replace("ik:binds \"report\" ;", ""));
}

#[test]
fn another_plans_fork_binding_does_not_satisfy_a_reference() {
    let mut data = FORK_BINDS.replace("ik:binds \"report\" ;", "");
    // A fully valid second plan binds the same name, but cannot supply this plan's name.
    data.push_str(&FORK_BINDS.replace("urn:plan:fork-binds", "urn:plan:other"));
    assert_unbound(&data);
}
