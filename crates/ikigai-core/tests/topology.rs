//! **Spaces have identity, and the arrangement is a resource** — the paper's §8.2
//! and Theorem 4(b), end to end (`docs/formalism/README.md` §1, "Metadata verb,
//! self-description", and R7.3; ledger #515). One test per claim: a space that
//! names itself is injected under its own name and shares one cache entry across
//! rebuilds; renaming a self-named space at injection is refused; a hit reports the
//! innermost named space on its path and every core combinator forwards the
//! report; a named corridor answers for an anonymous space and the trace says so
//! on every event; `urn:kernel:topology` renders the chain and every core
//! combinator as IRIs with ordered layers, answers the chain it is asked in, and is
//! keyed by it; and the paper's §12.5 check — is the personal family reachable
//! without passing the limiter? — is a walk over that graph, NO with the limiter
//! and YES without it.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, Capability, Confine, Description, EndpointSpace, Error, Exact, Fallback,
    FnEndpoint, Iri, Kernel, Limit, MetaRenderer, Mount, ReprType, Representation, Request,
    Resolution, Rewrite, Scope, Space, SpaceKind, TraceEvent, Tracer, UriTemplate, Verb,
    ANSWERED_NOTE, BINDINGS_THREAD, DENIED_NOTE, LIMITED_NOTE,
};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const IK: &str = "https://ikigai-rs.dev/ns#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// A cacheable constant with a description id of its own.
fn door(id: &'static str) -> FnEndpoint {
    FnEndpoint::new(id, move |_inv| Ok(text(id.as_bytes()).cacheable()))
        .with_description(Description::new(id).verb(Verb::Source))
}

/// A door that demands a scope, so a denial can be traced.
fn gated(id: &'static str, scope: &'static str) -> FnEndpoint {
    FnEndpoint::new(id, move |_inv| Ok(text(id.as_bytes()).cacheable()))
        .with_description(Description::new(id).verb(Verb::Source).requires(scope))
}

/// Renders a description as its id.
struct IdRenderer;

impl MetaRenderer for IdRenderer {
    fn render(
        &self,
        description: &Description,
        _target: &ReprType,
    ) -> ikigai_core::Result<Representation> {
        Ok(text(description.id.as_bytes()))
    }
}

fn kernel(root: Arc<dyn Space>) -> Kernel {
    Kernel::with_meta_renderer(root, Arc::new(IdRenderer))
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn note<'a>(event: &'a TraceEvent, key: &str) -> Option<&'a str> {
    event
        .notes
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A space that says nothing about itself: the default `topology()`.
struct Foreign(Arc<dyn Space>);
impl Space for Foreign {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        self.0.resolve(request, scope)
    }
}

// ---- 1. Identity on the space ------------------------------------------------

#[test]
fn a_space_that_names_itself_is_injected_under_its_own_name_and_shares_one_cache_entry() {
    let name = "urn:example:ctx:time:2026-09-25T18:00Z";
    let pinned = |named: bool| -> Arc<dyn Space> {
        let space = EndpointSpace::new().bind(Exact::new("urn:time:now"), door("pinned"));
        Arc::new(if named { space.named(iri(name)) } else { space })
    };
    assert_eq!(pinned(true).id().unwrap().as_str(), name);
    assert!(
        pinned(false).id().is_none(),
        "a space is anonymous unless it says otherwise"
    );

    let kernel = kernel(Arc::new(EndpointSpace::new()));
    let cap = Capability::root();
    let now = || source("urn:time:now");

    // Two requests, two freshly built self-named spaces, injected with `with`:
    // ONE entry, and the chain renders under the space's own name.
    let scope = || Scope::empty().with(pinned(true));
    assert_eq!(scope().to_string(), format!("{name} root"));
    block_on(kernel.issue_in(now(), &cap, scope())).unwrap();
    block_on(kernel.issue_in(now(), &cap, scope())).unwrap();
    assert_eq!(kernel.cache_len(), 1);
    assert!(kernel.is_cached_in(&now(), &cap, &scope()));

    // The same two requests through anonymous spaces: an entry each, as before.
    block_on(kernel.issue_in(now(), &cap, Scope::empty().with(pinned(false)))).unwrap();
    block_on(kernel.issue_in(now(), &cap, Scope::empty().with(pinned(false)))).unwrap();
    assert_eq!(kernel.cache_len(), 3);

    // `with_named` agreeing with the space's own name is the same corridor.
    block_on(kernel.issue_in(
        now(),
        &cap,
        Scope::empty().with_named(iri(name), pinned(true)),
    ))
    .unwrap();
    assert_eq!(kernel.cache_len(), 3);
}

#[test]
#[should_panic(expected = "a self-named space is injected under its own identity")]
fn renaming_a_self_named_space_at_injection_is_refused() {
    // The two-partitions bug, reintroduced on purpose: refused rather than
    // resolved either way (the space's name would drop what the injector's
    // carried; the injector's would put one set of doors under two names).
    let space: Arc<dyn Space> = Arc::new(EndpointSpace::new().named(iri("urn:example:space:a")));
    let _ = Scope::empty().with_named(iri("urn:example:space:b"), space);
}

#[test]
#[should_panic(expected = "a self-named space is injected under its own identity")]
fn confining_to_a_self_named_space_under_another_name_is_refused() {
    let space: Arc<dyn Space> = Arc::new(EndpointSpace::new().named(iri("urn:example:space:a")));
    let _ = Confine::new(iri("urn:example:space:b"), space, Arc::new(door("x")));
}

#[test]
fn every_core_combinator_can_be_named_and_reports_its_name() {
    let leaf = || -> Arc<dyn Space> { Arc::new(EndpointSpace::new()) };
    let named: Vec<(&str, Arc<dyn Space>)> = vec![
        (
            "EndpointSpace",
            Arc::new(EndpointSpace::new().named(iri("urn:example:s:1"))),
        ),
        (
            "Fallback",
            Arc::new(Fallback::new(vec![leaf()]).named(iri("urn:example:s:2"))),
        ),
        (
            "Mount",
            Arc::new(Mount::new("urn:x:", leaf()).named(iri("urn:example:s:3"))),
        ),
        (
            "Rewrite",
            Arc::new(Rewrite::new(leaf(), |_| None).named(iri("urn:example:s:4"))),
        ),
        (
            "Alias",
            Arc::new(Alias::new(Arc::new(AliasTable::new()), leaf()).named(iri("urn:example:s:5"))),
        ),
        (
            "Limit",
            Arc::new(Limit::new("urn:x:").named(iri("urn:example:s:6"))),
        ),
    ];
    for (i, (label, space)) in named.iter().enumerate() {
        assert_eq!(
            space.id().map(|id| id.as_str().to_string()),
            Some(format!("urn:example:s:{}", i + 1)),
            "{label} does not report the name it was given"
        );
        assert_eq!(
            space.topology().id,
            space.id(),
            "{label}'s node is not named by its id"
        );
    }
    // …and the blanket impl forwards it through an `Arc`.
    let erased: Arc<dyn Space> = Arc::clone(&named[0].1);
    assert_eq!(Arc::new(erased).id().unwrap().as_str(), "urn:example:s:1");
}

// ---- 2. Who answered -----------------------------------------------------------

#[test]
fn a_hit_reports_the_innermost_named_space_and_every_combinator_forwards_it() {
    let personal = || -> Arc<dyn Space> {
        Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:personal:x"), door("personal-x"))
                .named(iri("urn:example:space:personal")),
        )
    };
    let public = || -> Arc<dyn Space> {
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:public:y"), door("public-y")))
    };
    let answered = |space: &dyn Space, target: &str| -> Option<String> {
        match space.resolve(&source(target), &Scope::empty()) {
            Resolution::Hit(hit) => hit.answered_by.map(|id| id.as_str().to_string()),
            Resolution::Miss => panic!("{target} missed"),
        }
    };
    let s = |x: &str| Some(x.to_string());

    // A leaf reports itself; an anonymous leaf reports nothing.
    assert_eq!(
        answered(&*personal(), "urn:personal:x"),
        s("urn:example:space:personal")
    );
    assert_eq!(answered(&*public(), "urn:public:y"), None);

    // A named combinator FILLS an absence and never overwrites an inner report.
    let root = Fallback::new(vec![
        Arc::new(Mount::new("urn:personal:", personal()).named(iri("urn:example:space:m1"))),
        Arc::new(Mount::new("urn:public:", public()).named(iri("urn:example:space:m2"))),
    ])
    .named(iri("urn:example:space:root"));
    assert_eq!(
        answered(&root, "urn:personal:x"),
        s("urn:example:space:personal")
    );
    assert_eq!(answered(&root, "urn:public:y"), s("urn:example:space:m2"));

    // The same rule through every combinator that rewrites or wraps.
    let rewrite = |inner: Arc<dyn Space>| {
        Rewrite::new(inner, |t: &Iri| {
            t.as_str()
                .strip_prefix("urn:alias:")
                .map(|rest| iri(&format!("urn:personal:{rest}")))
        })
        .named(iri("urn:example:space:rewrite"))
    };
    assert_eq!(
        answered(&rewrite(personal()), "urn:alias:x"),
        s("urn:example:space:personal")
    );
    let anonymous_personal: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:personal:x"), door("personal-x")));
    assert_eq!(
        answered(&rewrite(Arc::clone(&anonymous_personal)), "urn:alias:x"),
        s("urn:example:space:rewrite")
    );
    let table = Arc::new(AliasTable::new().prefix("urn:alias:", "urn:personal:"));
    let alias = |inner: Arc<dyn Space>| {
        Alias::new(Arc::clone(&table), inner).named(iri("urn:example:space:alias"))
    };
    assert_eq!(
        answered(&alias(personal()), "urn:alias:x"),
        s("urn:example:space:personal")
    );
    assert_eq!(
        answered(&alias(Arc::clone(&anonymous_personal)), "urn:alias:x"),
        s("urn:example:space:alias")
    );
    // A named limiter names itself on the hit on ⊥.
    let limited = Fallback::new(vec![
        Arc::new(Limit::new("urn:personal:").named(iri("urn:example:space:limit"))),
        personal(),
    ]);
    match limited.resolve(&source("urn:personal:x"), &Scope::empty()) {
        Resolution::Hit(hit) => {
            assert!(hit.endpoint.is_limiter());
            assert_eq!(hit.answered_by.unwrap().as_str(), "urn:example:space:limit");
        }
        Resolution::Miss => panic!("in the family"),
    }
    // Decoration keeps it, as it keeps the canonical.
    let decorated = personal()
        .resolve(&source("urn:personal:x"), &Scope::empty())
        .map_endpoint(|endpoint| endpoint);
    match decorated {
        Resolution::Hit(hit) => {
            assert_eq!(
                hit.answered_by.as_ref().unwrap().as_str(),
                "urn:example:space:personal"
            );
            let swapped = hit.with_endpoint(Arc::new(door("other")));
            assert_eq!(
                swapped.answered_by.unwrap().as_str(),
                "urn:example:space:personal"
            );
        }
        Resolution::Miss => unreachable!(),
    }
}

#[test]
fn a_named_corridor_answers_for_an_anonymous_space_and_every_traced_event_says_who() {
    // Root: a named leaf with an open door and a gated one; an anonymous corridor
    // shadowing nothing but binding its own name.
    let root: Arc<dyn Space> = Arc::new(
        Fallback::new(vec![
            Arc::new(Limit::new("urn:hole:").named(iri("urn:example:space:hole"))),
            Arc::new(
                EndpointSpace::new()
                    .bind(Exact::new("urn:open"), door("open"))
                    .bind(
                        Exact::new("urn:gated"),
                        gated("gated", "urn:cap:example:gated"),
                    )
                    .named(iri("urn:example:space:root")),
            ),
        ])
        .named(iri("urn:example:space:top")),
    );
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:ctx:doc"), door("doc")));
    let kernel = kernel(root);
    let tracer = Arc::new(Collect::default());
    kernel.set_tracer(tracer.clone());
    let cap = Capability::root();
    let chain = || Scope::empty().with_named(iri("urn:example:ctx:7"), Arc::clone(&corridor));

    // A hit through the corridor names the corridor; through the root, the
    // innermost named space (the leaf, not the fallback around it).
    block_on(kernel.issue_in(source("urn:ctx:doc"), &cap, chain())).unwrap();
    block_on(kernel.issue_in(source("urn:open"), &cap, chain())).unwrap();
    // A cache hit carries it too.
    block_on(kernel.issue_in(source("urn:open"), &cap, chain())).unwrap();
    // A denial names the space whose door refused…
    let denied = block_on(kernel.issue(
        source("urn:gated"),
        &Capability::scoped(Vec::<String>::new()),
    ))
    .unwrap_err();
    assert!(matches!(denied, Error::Denied(_)));
    // …and a hit on a named limiter names the limiter.
    let limited = block_on(kernel.issue(source("urn:hole:x"), &cap)).unwrap_err();
    assert!(matches!(limited, Error::Unresolved(_)));

    let events = tracer.0.lock().unwrap();
    let by_target = |target: &str| -> Vec<&TraceEvent> {
        events.iter().filter(|e| e.target == target).collect()
    };
    assert_eq!(
        note(by_target("urn:ctx:doc")[0], ANSWERED_NOTE),
        Some("urn:example:ctx:7"),
        "{events:?}"
    );
    let open = by_target("urn:open");
    assert_eq!(open.len(), 2);
    assert!(!open[0].cache_hit && open[1].cache_hit);
    for event in &open {
        assert_eq!(note(event, ANSWERED_NOTE), Some("urn:example:space:root"));
    }
    let denied = by_target("urn:gated");
    assert_eq!(note(denied[0], DENIED_NOTE), Some("urn:cap:example:gated"));
    assert_eq!(
        note(denied[0], ANSWERED_NOTE),
        Some("urn:example:space:root")
    );
    let limited = by_target("urn:hole:x");
    assert_eq!(note(limited[0], LIMITED_NOTE), Some("urn:hole:x"));
    assert_eq!(
        note(limited[0], ANSWERED_NOTE),
        Some("urn:example:space:hole")
    );
    drop(events);

    // And a kernel whose spaces claim nothing records no such note at all.
    let plain = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:open"), door("open")),
    ));
    let quiet = Arc::new(Collect::default());
    plain.set_tracer(quiet.clone());
    block_on(plain.issue(source("urn:open"), &cap)).unwrap();
    block_on(plain.issue_in(
        source("urn:open"),
        &cap,
        Scope::empty().with(Arc::clone(&corridor)),
    ))
    .unwrap();
    for event in quiet.0.lock().unwrap().iter() {
        assert_eq!(note(event, ANSWERED_NOTE), None, "{event:?}");
    }
}

// ---- 3. The arrangement as a resource ----------------------------------------

/// A parsed topology graph, walked by IRI.
struct Graph(Vec<Triple>);

impl Graph {
    fn parse(turtle: &str) -> Graph {
        let triples: Vec<Triple> = oxttl::TurtleParser::new()
            .for_reader(turtle.as_bytes())
            .map(|t| t.unwrap_or_else(|e| panic!("{e}\n{turtle}")))
            .collect();
        for t in &triples {
            assert!(
                matches!(t.subject, NamedOrBlankNode::NamedNode(_))
                    && !matches!(t.object, Term::BlankNode(_)),
                "a blank node in the topology: {t}"
            );
        }
        Graph(triples)
    }

    fn objects(&self, s: &str, p: &str) -> Vec<&Term> {
        self.0
            .iter()
            .filter(|t| matches!(&t.subject, NamedOrBlankNode::NamedNode(n) if n.as_str() == s))
            .filter(|t| t.predicate.as_str() == p)
            .map(|t| &t.object)
            .collect()
    }

    fn iri(&self, s: &str, p: &str) -> Option<String> {
        self.objects(s, p).first().map(|o| match o {
            Term::NamedNode(n) => n.as_str().to_string(),
            other => panic!("{s} {p} is not an IRI: {other}"),
        })
    }

    fn strs(&self, s: &str, p: &str) -> Vec<String> {
        self.objects(s, p)
            .into_iter()
            .map(|o| match o {
                Term::Literal(l) => l.value().to_string(),
                other => panic!("{s} {p} is not a literal: {other}"),
            })
            .collect()
    }

    fn kind(&self, s: &str) -> String {
        let types = self
            .iri(s, &format!("{RDF}type"))
            .unwrap_or_else(|| panic!("{s} has no type"));
        types.strip_prefix(IK).expect("an ik: kind").to_string()
    }

    /// The members of `s`'s `ik:layers`, in list order.
    fn layers(&self, s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cell = self
            .iri(s, &format!("{IK}layers"))
            .unwrap_or_else(|| panic!("{s} has no layers"));
        while cell != format!("{RDF}nil") {
            out.push(self.iri(&cell, &format!("{RDF}first")).expect("rdf:first"));
            cell = self.iri(&cell, &format!("{RDF}rest")).expect("rdf:rest");
        }
        out
    }

    fn space(&self, s: &str) -> String {
        self.iri(s, &format!("{IK}space"))
            .unwrap_or_else(|| panic!("{s} encloses nothing"))
    }

    fn subjects(&self) -> BTreeSet<String> {
        self.0
            .iter()
            .map(|t| match &t.subject {
                NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
                _ => unreachable!(),
            })
            .collect()
    }
}

fn topology(kernel: &Kernel, cap: &Capability, scope: Scope) -> Graph {
    let rep = block_on(kernel.issue_in(source("urn:kernel:topology"), cap, scope)).unwrap();
    assert_eq!(rep.repr_type.media_type, "text/turtle");
    Graph::parse(std::str::from_utf8(&rep.bytes).unwrap())
}

#[test]
fn the_topology_renders_the_chain_and_every_core_combinator_as_iris_with_ordered_layers() {
    let table = Arc::new(
        AliasTable::new()
            .prefix("urn:alias:", "urn:personal:")
            .exact("urn:old", "urn:public:y"),
    );
    let root: Arc<dyn Space> = Arc::new(
        Fallback::new(vec![
            Arc::new(Limit::matching(
                UriTemplate::parse("urn:doc:{id}:secret").unwrap(),
            )),
            Arc::new(Mount::new(
                "urn:personal:",
                Arc::new(
                    EndpointSpace::new()
                        .bind(Exact::new("urn:personal:x"), door("personal-x"))
                        .bind(
                            UriTemplate::parse("urn:personal:doc/{id}").unwrap(),
                            door("personal-doc"),
                        )
                        .named(iri("urn:example:space:personal")),
                ),
            )),
            Arc::new(Alias::new(
                Arc::clone(&table),
                Arc::new(Rewrite::new(
                    Arc::new(Foreign(Arc::new(EndpointSpace::new()))),
                    |_| None,
                )),
            )),
        ])
        .named(iri("urn:example:space:root")),
    );
    let kernel = kernel(Arc::clone(&root));
    let cap = Capability::root();

    // The empty chain: one layer, the root, under its own name.
    let g = topology(&kernel, &cap, Scope::empty());
    assert_eq!(g.kind("urn:ikigai:chain:root"), "Chain");
    assert_eq!(
        g.strs("urn:ikigai:chain:root", &format!("{IK}severed")),
        ["false"]
    );
    assert_eq!(
        g.layers("urn:ikigai:chain:root"),
        ["urn:example:space:root"]
    );
    // The root fallback: three layers in consultation order, anonymous ones
    // skolemized in pre-order.
    assert_eq!(g.kind("urn:example:space:root"), "Fallback");
    let layers = g.layers("urn:example:space:root");
    assert_eq!(
        layers,
        [
            "urn:ikigai:space:_:1",
            "urn:ikigai:space:_:2",
            "urn:ikigai:space:_:3"
        ]
    );
    assert_eq!(g.kind(&layers[0]), "Limit");
    assert_eq!(
        g.strs(&layers[0], &format!("{IK}family")),
        ["urn:doc:{id}:secret"]
    );
    assert_eq!(g.kind(&layers[1]), "Mount");
    assert_eq!(
        g.strs(&layers[1], &format!("{IK}prefix")),
        ["urn:personal:"]
    );
    assert_eq!(g.space(&layers[1]), "urn:example:space:personal");
    assert_eq!(g.kind("urn:example:space:personal"), "EndpointSpace");
    let mut patterns = g.strs("urn:example:space:personal", &format!("{IK}pattern"));
    patterns.sort();
    assert_eq!(patterns, ["urn:personal:doc/{id}", "urn:personal:x"]);
    // The alias carries its table; the rewrite encloses and says nothing of τ; the
    // foreign space is opaque — the graph says where knowledge stops.
    assert_eq!(g.kind(&layers[2]), "Alias");
    let rules: Vec<String> = g
        .objects(&layers[2], &format!("{IK}rewrites"))
        .iter()
        .map(|o| o.to_string())
        .collect();
    assert_eq!(
        rules,
        [
            "<urn:ikigai:space:_:3:rule:1>",
            "<urn:ikigai:space:_:3:rule:2>"
        ]
    );
    assert_eq!(g.kind("urn:ikigai:space:_:3:rule:1"), "RewriteRule");
    assert_eq!(
        g.strs("urn:ikigai:space:_:3:rule:1", &format!("{IK}ruleKind")),
        ["prefix"]
    );
    assert_eq!(
        g.strs("urn:ikigai:space:_:3:rule:1", &format!("{IK}logical")),
        ["urn:alias:"]
    );
    assert_eq!(
        g.strs("urn:ikigai:space:_:3:rule:1", &format!("{IK}canonical")),
        ["urn:personal:"]
    );
    assert_eq!(
        g.strs("urn:ikigai:space:_:3:rule:2", &format!("{IK}ruleKind")),
        ["exact"]
    );
    let rewrite = g.space(&layers[2]);
    assert_eq!(g.kind(&rewrite), "Rewrite");
    assert_eq!(g.kind(&g.space(&rewrite)), "OpaqueSpace");
    // The Rust face is the same tree.
    let tree = kernel.topology();
    assert!(matches!(tree.kind, SpaceKind::Chain { severed: false }));
    assert_eq!(tree.children.len(), 1);
    assert!(matches!(tree.children[0].kind, SpaceKind::Fallback));
    assert_eq!(tree.children[0].children.len(), 3);

    // Inside a chain, the resource answers THE CHAIN: a named corridor first,
    // an anonymous one skolemized, then the root — and severed, no root.
    let corridor = |name: Option<&str>| -> Arc<dyn Space> {
        let space = EndpointSpace::new().bind(Exact::new("urn:ctx:doc"), door("doc"));
        Arc::new(match name {
            Some(name) => space.named(iri(name)),
            None => space,
        })
    };
    let chain = Scope::empty()
        .with(corridor(None))
        .with_named(iri("urn:example:ctx:7"), corridor(None));
    let g = topology(&kernel, &cap, chain.clone());
    let entry = format!("urn:ikigai:chain:{:016x}", chain.fingerprint());
    assert_eq!(g.kind(&entry), "Chain");
    assert_eq!(
        g.layers(&entry),
        [
            "urn:example:ctx:7",
            "urn:ikigai:space:_:1",
            "urn:example:space:root"
        ]
    );
    assert_eq!(g.kind("urn:example:ctx:7"), "EndpointSpace");
    let severed = chain.clone().sever();
    let g = topology(&kernel, &cap, severed.clone());
    let entry = format!("urn:ikigai:chain:{:016x}", severed.fingerprint());
    assert_eq!(g.strs(&entry, &format!("{IK}severed")), ["true"]);
    assert_eq!(
        g.layers(&entry),
        ["urn:example:ctx:7", "urn:ikigai:space:_:1"]
    );
    assert!(
        !g.subjects().contains("urn:example:space:root"),
        "a severed chain has no root"
    );

    // Keyed by the chain (three chains, three entries), hanging from the bindings
    // thread (a cut forgets all three), and gated like the catalog.
    assert_eq!(kernel.cache_len(), 3);
    assert!(kernel.is_cached_in(&source("urn:kernel:topology"), &cap, &chain));
    assert!(kernel.is_cached_in(&source("urn:kernel:topology"), &cap, &Scope::empty()));
    kernel.cut(BINDINGS_THREAD);
    assert!(!kernel.is_cached_in(&source("urn:kernel:topology"), &cap, &chain));
    assert!(!kernel.is_cached_in(&source("urn:kernel:topology"), &cap, &Scope::empty()));
    let denied = block_on(kernel.issue(
        source("urn:kernel:topology"),
        &Capability::scoped(["urn:cap:kernel:cut"]),
    ))
    .unwrap_err();
    assert!(matches!(denied, Error::Denied(_)));
    assert!(block_on(kernel.issue(
        source("urn:kernel:topology"),
        &Capability::scoped(["urn:cap:kernel:inspect"]),
    ))
    .is_ok());
}

/// The answer a reachability walk gives through a graph that may say "I cannot
/// see in here" (`ik:OpaqueSpace`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Reach {
    Reachable,
    Unreachable,
    Unknown,
}

/// **Theorem 4(b) as a walk** — the formal document's §1.1: ikigai's tree contributes
/// no pushes, so gatekeeper completeness over it is a path query, not a pushdown
/// analysis. Is any door of `family` reachable from `node` without a limiter over
/// the family standing ahead of it in some ordered list on the path?
///
/// The SPARQL form of the same walk is in `docs/formalism/README.md` (R7.3).
fn reach(g: &Graph, node: &str, family: &str) -> Reach {
    let p = |name: &str| format!("{IK}{name}");
    match g.kind(node).as_str() {
        "Chain" | "Fallback" => {
            let mut unknown = false;
            for layer in g.layers(node) {
                if g.kind(&layer) == "Limit"
                    && g.strs(&layer, &p("family"))
                        .iter()
                        .any(|limited| family.starts_with(limited.as_str()))
                {
                    // The hole covers the whole family: nothing after it is reached.
                    return Reach::Unreachable;
                }
                match reach(g, &layer, family) {
                    Reach::Reachable => return Reach::Reachable,
                    Reach::Unknown => unknown = true,
                    Reach::Unreachable => {}
                }
            }
            if unknown {
                Reach::Unknown
            } else {
                Reach::Unreachable
            }
        }
        "Mount" => {
            let prefix = g.strs(node, &p("prefix")).remove(0);
            if family.starts_with(&prefix) || prefix.starts_with(family) {
                reach(g, &g.space(node), family)
            } else {
                Reach::Unreachable
            }
        }
        "EndpointSpace" => {
            if g.strs(node, &p("pattern"))
                .iter()
                .any(|pattern| pattern.starts_with(family))
            {
                Reach::Reachable
            } else {
                Reach::Unreachable
            }
        }
        // A limiter reached on its own admits nothing; a rewrite, alias or
        // confinement is walked through (a rewrite INTO the family is the alias's
        // rules' business, and this walk over-reports rather than under-reports);
        // an opaque space is exactly that.
        "Limit" => Reach::Unreachable,
        "Rewrite" | "Alias" | "Confine" => reach(g, &g.space(node), family),
        "OpaqueSpace" => Reach::Unknown,
        other => panic!("unknown node kind {other}"),
    }
}

#[test]
fn the_papers_12_5_check_is_a_walk_over_the_topology_no_with_the_limiter_and_yes_without() {
    // The served root: `Fallback([Limit("urn:personal:"), root])`, where `root`
    // mounts a personal space and a public one.
    let root = || -> Arc<dyn Space> {
        Arc::new(
            Fallback::new(vec![
                Arc::new(Mount::new(
                    "urn:personal:",
                    Arc::new(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:personal:calendar"), door("calendar"))
                            .named(iri("urn:example:space:personal")),
                    ),
                )),
                Arc::new(Mount::new(
                    "urn:public:",
                    Arc::new(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:public:hello"), door("hello"))
                            .named(iri("urn:example:space:public")),
                    ),
                )),
            ])
            .named(iri("urn:example:space:root")),
        )
    };
    let served: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(Limit::new("urn:personal:").named(iri("urn:example:space:gatekeeper"))),
        root(),
    ]));
    let cap = Capability::root();

    // With the limiter: the personal family is NOT reachable from the entry, the
    // public one is — and the graph says so before any request is made.
    let g = topology(&kernel(served), &cap, Scope::empty());
    assert_eq!(
        reach(&g, "urn:ikigai:chain:root", "urn:personal:"),
        Reach::Unreachable
    );
    assert_eq!(
        reach(&g, "urn:ikigai:chain:root", "urn:public:"),
        Reach::Reachable
    );

    // Remove the limiter: reachable.
    let g = topology(&kernel(root()), &cap, Scope::empty());
    assert_eq!(
        reach(&g, "urn:ikigai:chain:root", "urn:personal:"),
        Reach::Reachable
    );

    // The limiter at a position — injected as a corridor — is a layer of the
    // chain, and the same walk finds it.
    let limited = Scope::empty().with(Arc::new(
        Limit::new("urn:personal:").named(iri("urn:example:space:gatekeeper")),
    ));
    let entry = format!("urn:ikigai:chain:{:016x}", limited.fingerprint());
    let g = topology(&kernel(root()), &cap, limited);
    assert_eq!(reach(&g, &entry, "urn:personal:"), Reach::Unreachable);
    assert_eq!(reach(&g, &entry, "urn:public:"), Reach::Reachable);

    // A limiter over a NARROWER family does not close the whole family, and a
    // foreign space on the path makes the answer "unknown", never "no".
    let partial: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(Limit::new("urn:personal:calendar")),
        Arc::new(Foreign(root())),
    ]));
    let g = topology(&kernel(partial), &cap, Scope::empty());
    assert_eq!(
        reach(&g, "urn:ikigai:chain:root", "urn:personal:"),
        Reach::Unknown
    );
}

#[test]
fn a_confinement_reports_the_corridor_it_severs_into() {
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:doc:1"), door("doc")));
    let confined = Confine::new(iri("urn:example:ctx:doc:1"), corridor, Arc::new(door("x")));
    let tree = confined.topology();
    assert!(matches!(tree.kind, SpaceKind::Confine));
    assert_eq!(tree.id.as_ref().unwrap().as_str(), "urn:example:ctx:doc:1");
    assert!(
        matches!(&tree.children[0].kind, SpaceKind::EndpointSpace { patterns } if patterns == &["urn:doc:1"])
    );
    let g = Graph::parse(&tree.to_turtle());
    assert_eq!(g.kind("urn:example:ctx:doc:1"), "Confine");
    assert_eq!(g.kind(&g.space("urn:example:ctx:doc:1")), "EndpointSpace");
}
