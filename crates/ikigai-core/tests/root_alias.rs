//! **A declared alias whose target resolves from the root** (ledger #2061): a VIEW.
//!
//! A child-resolving `ik:Alias` rewrites a name and resolves the result only in its own
//! child, so a dynamic space placed last in a host's `Fallback` could not declare a
//! view onto a name the host serves elsewhere: the #2061 reproduction answered
//! `Unresolved`. A rule marked `ik:resolveAt "root"` forwards the rewritten name from
//! the root, as a sub-request under the caller's own capability. One test per
//! acceptance criterion of the brief:
//!
//! 1. the #2061 reproduction answers the earlier member's value;
//! 2. under a capability that may not read the target, the view is refused exactly as
//!    the target is;
//! 3. a name an earlier member answers is never reached through a view;
//! 4. cutting the target's thread invalidates the cached view, and so does swapping
//!    the declaration;
//! 5. a self-loop and a two-step cycle are refused at build, and a cycle through two
//!    tables by the depth bound, never a hang;
//! 6. the topology renders the rule, `from_turtle` reads it back, `build` builds it;
//! 7. a child-resolving `ik:Alias` behaves exactly as before.
#![cfg(feature = "declare")]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    build, Alias, AliasTable, Capability, DeclarationError, Description, DynamicRefusal,
    DynamicSpace, EndpointSpace, Error, Exact, Fallback, FnEndpoint, Iri, Kernel, Registry,
    ReprType, Representation, Request, ResolveAt, RuleKind, Space, SpaceKind, Topology, Verb,
};

const TARGET: &str = "urn:example:ledger:open";
const VIEW: &str = "urn:example:view:open";
const READ: &str = "urn:cap:example:ledger:read";

/// What the gonk views arc declares: one view onto a name the host serves. Hand-written,
/// as a host's declaration resource would be, not rendered.
const VIEWS: &str = r#"
@prefix ik: <https://ikigai-rs.dev/ns#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .

<urn:ikigai:space:_:1> a ik:Alias ;
    ik:maxHops 8 ;
    ik:rewrites <urn:ikigai:space:_:1:rule:1> ;
    ik:space <urn:ikigai:space:_:2> .

<urn:ikigai:space:_:1:rule:1> a ik:RewriteRule ;
    ik:ruleKind "exact" ;
    ik:logical "urn:example:view:open" ;
    ik:canonical "urn:example:ledger:open" ;
    ik:resolveAt "root" .

<urn:ikigai:space:_:2> a ik:EndpointSpace ;
    ik:doors rdf:nil .
"#;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// The host's door at `urn:example:ledger:open`: cacheable, counting its runs, and
/// declaring the read capability a caller must hold.
fn open(runs: Arc<AtomicU32>) -> FnEndpoint {
    FnEndpoint::new("open", move |_| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(ReprType::new("text/plain"), b"open items".to_vec()).cacheable())
    })
    .with_description(Description::new("open").verb(Verb::Source).requires(READ))
}

struct Hub {
    kernel: Kernel,
    dynamic: Arc<DynamicSpace>,
    runs: Arc<AtomicU32>,
}

/// The #2061 arrangement: `Fallback[EndpointSpace{ledger:open, extra…}, DynamicSpace("views")]`.
fn hub(extra: impl FnOnce(EndpointSpace) -> EndpointSpace) -> Hub {
    let runs = Arc::new(AtomicU32::new(0));
    let host = extra(EndpointSpace::new().bind(Exact::new(TARGET), open(Arc::clone(&runs))));
    let dynamic = Arc::new(DynamicSpace::new(
        "views",
        iri("urn:example:decl:views"),
        Registry::new(),
    ));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(host), dynamic.clone()]));
    Hub {
        kernel: Kernel::new(root),
        dynamic,
        runs,
    }
}

impl Hub {
    fn install(&self, turtle: &str) -> Result<(), DynamicRefusal> {
        block_on(self.dynamic.install(&self.kernel, turtle))
    }

    fn get(&self, target: &str, cap: &Capability) -> Result<Vec<u8>, Error> {
        block_on(self.kernel.issue(source(target), cap)).map(|r| r.bytes)
    }
}

/// A one-alias declaration of `rules` (kind, from, to, at), over an empty child.
fn declaration(rules: &[(&str, &str, ResolveAt)]) -> String {
    let table = rules.iter().fold(AliasTable::new(), |t, (from, to, at)| {
        t.rule_at(RuleKind::Exact, *from, *to, *at)
    });
    Alias::new(Arc::new(table), Arc::new(EndpointSpace::new()))
        .topology()
        .to_turtle()
}

// 1 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_2061_reproduction_answers_the_earlier_members_value() {
    let hub = hub(|s| s);
    hub.install(VIEWS).unwrap();
    assert_eq!(
        hub.get(VIEW, &Capability::root()).unwrap(),
        b"open items",
        "a root-resolving view in a DynamicSpace last in a Fallback forwards to the host"
    );
    // The same declaration with the rule left a child rule is the #2061 behavior.
    hub.install(&VIEWS.replace(" ;\n    ik:resolveAt \"root\"", ""))
        .unwrap();
    assert!(matches!(
        hub.get(VIEW, &Capability::root()),
        Err(Error::Unresolved(_))
    ));
}

// 2 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_view_grants_nothing() {
    let hub = hub(|s| s);
    hub.install(VIEWS).unwrap();
    let reader = Capability::root().attenuate([READ]);
    assert_eq!(hub.get(VIEW, &reader).unwrap(), b"open items");

    let stranger = Capability::root().attenuate(["urn:cap:example:other"]);
    let direct = hub.get(TARGET, &stranger).unwrap_err();
    let through = hub.get(VIEW, &stranger).unwrap_err();
    assert!(matches!(direct, Error::Denied(_)), "{direct}");
    assert!(matches!(through, Error::Denied(_)), "{through}");
    assert_eq!(
        through.to_string(),
        direct.to_string(),
        "refused exactly as the target is"
    );
}

// 3 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_view_never_stands_in_for_a_name_an_earlier_member_answers() {
    let hub = hub(|s| {
        s.bind(
            Exact::new(VIEW),
            FnEndpoint::new("real", |_| {
                Ok(Representation::new(
                    ReprType::new("text/plain"),
                    b"real".to_vec(),
                ))
            }),
        )
    });
    hub.install(VIEWS).unwrap();
    assert_eq!(hub.get(VIEW, &Capability::root()).unwrap(), b"real");
    assert_eq!(hub.runs.load(Ordering::SeqCst), 0, "the target never ran");
}

// 4 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_cut_to_the_target_or_a_swap_invalidates_the_view() {
    let hub = hub(|s| s);
    hub.install(VIEWS).unwrap();
    let root = Capability::root();
    let cached = |target: &str| hub.kernel.is_cached(&source(target), &root);

    hub.get(VIEW, &root).unwrap();
    assert!(cached(VIEW), "the view's answer is cached");
    hub.get(VIEW, &root).unwrap();
    assert_eq!(hub.runs.load(Ordering::SeqCst), 1, "served from the cache");

    // Its own entry, not the target's: dropping the target's entry by its thread
    // takes the view's with it.
    hub.kernel.cut(TARGET);
    assert!(!cached(VIEW), "a cut to the target invalidates the view");
    hub.get(VIEW, &root).unwrap();
    assert_eq!(hub.runs.load(Ordering::SeqCst), 2);
    assert!(cached(VIEW));

    // Swapping the declaration cuts the dynamic space's thread.
    hub.install(VIEWS).unwrap();
    assert!(
        !cached(VIEW),
        "a swap of the declaration invalidates the view"
    );
    assert!(cached(TARGET), "and leaves the target's own entry alone");
}

// 5 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_self_loop_and_a_two_step_cycle_are_refused_at_build() {
    let hub = hub(|s| s);
    for rules in [
        vec![("urn:example:view:a", "urn:example:view:a", ResolveAt::Root)],
        vec![
            ("urn:example:view:a", "urn:example:view:b", ResolveAt::Root),
            ("urn:example:view:b", "urn:example:view:a", ResolveAt::Root),
        ],
    ] {
        let refusal = hub.install(&declaration(&rules)).unwrap_err();
        let DynamicRefusal::Declaration(DeclarationError::Malformed { node, reason }) = &refusal
        else {
            panic!("refused as malformed, got {refusal}");
        };
        assert!(node.as_deref().unwrap().contains(":rule:"), "{refusal}");
        assert!(reason.contains("alias cycle"), "{refusal}");
    }
    assert_eq!(hub.dynamic.status().installs, 0, "nothing bound");
}

#[test]
fn a_cycle_through_two_tables_stops_at_the_depth_bound() {
    // A view onto a view onto the first: no one table shows the loop, so the kernel's
    // nesting-depth bound stops it.
    let view = |from: &str, to: &str| -> Arc<dyn Space> {
        Arc::new(Alias::new(
            Arc::new(AliasTable::new().exact_at_root(from, to)),
            Arc::new(EndpointSpace::new()),
        ))
    };
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        view("urn:example:view:a", "urn:example:view:b"),
        view("urn:example:view:b", "urn:example:view:a"),
    ]));
    let kernel = Kernel::new(root);
    let err =
        block_on(kernel.issue(source("urn:example:view:a"), &Capability::root())).unwrap_err();
    assert!(matches!(err, Error::DepthExceeded { .. }), "{err}");
}

// 6 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_topology_renders_the_rule_and_reads_it_back() {
    let alias = Alias::new(
        Arc::new(
            AliasTable::new()
                .exact_at_root(VIEW, TARGET)
                .prefix("urn:example:old:", "urn:example:new:"),
        ),
        Arc::new(EndpointSpace::new()),
    );
    let topology = alias.topology();
    let turtle = topology.to_turtle();
    assert_eq!(
        turtle.matches("ik:resolveAt \"root\"").count(),
        1,
        "only the view says where it resolves:\n{turtle}"
    );
    let back = Topology::from_turtle(&turtle).unwrap();
    assert_eq!(back, topology, "round trip");
    let SpaceKind::Alias { rules, .. } = &back.kind else {
        panic!("an alias");
    };
    let at: Vec<ResolveAt> = rules.iter().map(|rule| rule.at).collect();
    assert_eq!(at, [ResolveAt::Root, ResolveAt::Child]);

    // `build` builds it, and what it builds forwards.
    let built = build(&back, &Registry::new()).unwrap();
    assert_eq!(built.topology(), topology);
    let hub = hub(|s| s);
    hub.install(&turtle).unwrap();
    assert_eq!(hub.get(VIEW, &Capability::root()).unwrap(), b"open items");

    // The hand-written declaration reads as the same rule.
    let decl = Topology::from_turtle(VIEWS).unwrap();
    let SpaceKind::Alias { rules, .. } = &decl.kind else {
        panic!("an alias");
    };
    assert_eq!(rules[0].at, ResolveAt::Root);

    // A value other than child or root is refused, naming the rule.
    let bad = VIEWS.replace("ik:resolveAt \"root\"", "ik:resolveAt \"level\"");
    let err = Topology::from_turtle(&bad).unwrap_err();
    assert!(err.to_string().contains("not child or root"), "{err}");
}

#[test]
fn the_text_form_carries_the_place_a_rule_resolves() {
    let text = "exact urn:example:view:open urn:example:ledger:open root\n\
                prefix urn:example:old: urn:example:new:\n";
    let table = AliasTable::parse(text).unwrap();
    assert_eq!(table.rules()[0].resolve_at(), ResolveAt::Root);
    assert_eq!(table.rules()[1].resolve_at(), ResolveAt::Child);
    assert_eq!(
        table.to_text(),
        text,
        "round trip; a child rule writes no keyword"
    );
    assert!(AliasTable::parse("exact urn:a urn:b sideways").is_err());
}

// 7 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_child_resolving_alias_behaves_exactly_as_before() {
    // It renders with no ik:resolveAt…
    let table = AliasTable::new().exact("urn:example:old", "urn:example:new");
    let child: Arc<dyn Space> = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:example:new"),
        FnEndpoint::new("new", |_| {
            Ok(Representation::new(ReprType::new("text/plain"), b"new".to_vec()).cacheable())
        }),
    ));
    let alias = Alias::new(Arc::new(table), child);
    assert!(!alias.topology().to_turtle().contains("resolveAt"));

    // …resolves in its child, and the two names share one cache entry (the rewrite
    // is reported as the canonical name).
    let kernel = Kernel::new(Arc::new(alias));
    let root = Capability::root();
    assert_eq!(
        block_on(kernel.issue(source("urn:example:old"), &root))
            .unwrap()
            .bytes,
        b"new"
    );
    assert!(kernel.is_cached(&source("urn:example:new"), &root));
    assert_eq!(kernel.cache_len(), 1);
}
