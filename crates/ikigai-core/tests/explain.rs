//! **`urn:kernel:explain` — the module author's "why did my endpoint not get
//! called?"** (ledger #906). Each test is one question an author asks, answered by
//! name: a grammar miss, an alias onto nothing, a capability refusal, an opaque
//! space, a found name, a shadowed door. And the properties that make it safe to
//! ask: it invokes nothing, it moves no counter, it is cached and a binding change
//! recomputes it, and `scopes=` answers for a NARROWER capability, never a wider one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AliasTable, ArgRef, Capability, Description, EndpointSpace, Error, Exact, Fallback, FnEndpoint,
    Iri, Kernel, Mount, ReprType, Representation, Request, Resolution, Scope, Space, UriTemplate,
    Verb,
};

const INSPECT: &str = "urn:cap:kernel:inspect";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec()).cacheable()
}

/// An endpoint that counts its invocations — the proof a dry run is dry.
fn counting(name: &'static str, calls: &Arc<AtomicUsize>) -> FnEndpoint {
    let calls = Arc::clone(calls);
    FnEndpoint::new(name, move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(text(name))
    })
}

fn explain_request(target: &str, args: &[(&str, &str)]) -> Request {
    let mut request = Request::new(Verb::Source, iri("urn:kernel:explain"))
        .with_arg("target", ArgRef::Inline(target.as_bytes().to_vec()));
    for (name, value) in args {
        request = request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec()));
    }
    request
}

fn explain_in(
    kernel: &Kernel,
    target: &str,
    args: &[(&str, &str)],
    capability: &Capability,
    scope: Scope,
) -> Result<String, Error> {
    block_on(kernel.issue_in(explain_request(target, args), capability, scope))
        .map(|r| String::from_utf8(r.bytes).unwrap())
}

fn explain(kernel: &Kernel, target: &str, args: &[(&str, &str)]) -> String {
    explain_in(kernel, target, args, &Capability::root(), Scope::empty()).unwrap()
}

/// The status word on the member line labeled `label`.
fn member_line<'a>(explanation: &'a str, label: &str) -> &'a str {
    explanation
        .lines()
        .find(|l| {
            let t = l.trim_start();
            t.split_once(". ")
                .is_some_and(|(n, rest)| n.parse::<usize>().is_ok() && rest.starts_with(label))
        })
        .unwrap_or_else(|| panic!("no member line for `{label}` in:\n{explanation}"))
}

fn greeter_root(calls: &Arc<AtomicUsize>) -> Arc<dyn Space> {
    Arc::new(
        EndpointSpace::new()
            .bind(
                UriTemplate::parse("urn:t:greet:{name}").unwrap(),
                counting("greeter", calls),
            )
            .bind(Exact::new("urn:t:plain"), counting("plain", calls)),
    )
}

/// A space that reports no structure: the default `Space::topology`.
struct Opaque(Arc<dyn Space>);
impl Space for Opaque {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        self.0.resolve(request, scope)
    }
}

/// A space that counts how often it is asked to resolve — the proof an explanation
/// was served from the cache rather than walked again.
struct CountingSpace {
    inner: Arc<dyn Space>,
    resolves: Arc<AtomicUsize>,
}
impl Space for CountingSpace {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve(request, scope)
    }
    fn topology(&self) -> ikigai_core::Topology {
        self.inner.topology()
    }
}

#[test]
fn a_grammar_miss_shows_every_space_declining_and_nothing_answering() {
    let calls = Arc::new(AtomicUsize::new(0));
    let kernel = Kernel::new(greeter_root(&calls));
    let out = explain(&kernel, "urn:t:greet", &[]);
    assert!(member_line(&out, "root").contains("declined"), "{out}");
    assert!(
        out.contains("unresolved: nothing in the chain answers this name"),
        "{out}"
    );
    assert!(
        !out.contains("endpoint "),
        "a miss names no endpoint:\n{out}"
    );
    // …and in a chain, EVERY member declines, in the order it is consulted.
    let corridor =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:t:other"), counting("o", &calls)));
    let out = explain_in(
        &kernel,
        "urn:t:greet",
        &[],
        &Capability::root(),
        Scope::empty().with_named(iri("urn:ctx:c"), corridor),
    )
    .unwrap();
    let first = member_line(&out, "urn:ctx:c");
    let second = member_line(&out, "root");
    assert!(
        first.contains("declined") && second.contains("declined"),
        "{out}"
    );
    assert!(
        out.find(first).unwrap() < out.find(second).unwrap(),
        "{out}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_alias_onto_nothing_shows_the_rule_that_fired_and_moves_no_counter() {
    let calls = Arc::new(AtomicUsize::new(0));
    let table = Arc::new(AliasTable::new().prefix("urn:old:greet:", "urn:gone:greet:"));
    let kernel = Kernel::new(greeter_root(&calls)).with_aliases(Arc::clone(&table));
    let out = explain(&kernel, "urn:old:greet:alice", &[]);
    assert!(
        out.contains("rewrite     prefix urn:old:greet: -> urn:gone:greet:"),
        "{out}"
    );
    assert!(out.contains("resolves as urn:gone:greet:alice"), "{out}");
    assert!(member_line(&out, "root").contains("declined"), "{out}");
    assert!(out.contains("moved the name onto nothing"), "{out}");
    // A dry run is not a resolution: the counters urn:kernel:aliases reads are
    // untouched, so asking why a name misses does not itself count as a miss.
    let rule = &table.rules()[0];
    assert_eq!((rule.hops(), rule.unresolved(), rule.refused()), (0, 0, 0));
    // The real request does count, which is what the readout is for.
    let real = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:old:greet:alice")),
        &Capability::root(),
    ));
    assert!(matches!(real, Err(Error::Unresolved(_))), "{real:?}");
    assert_eq!((rule.hops(), rule.unresolved()), (1, 1));
}

#[test]
fn a_rewrite_onto_a_bound_name_explains_the_backing_endpoint() {
    let calls = Arc::new(AtomicUsize::new(0));
    let table = Arc::new(AliasTable::new().exact("urn:t:hello", "urn:t:greet:world"));
    let kernel = Kernel::new(greeter_root(&calls)).with_aliases(table);
    let out = explain(&kernel, "urn:t:hello", &[]);
    assert!(
        out.contains("rewrite     exact urn:t:hello -> urn:t:greet:world"),
        "{out}"
    );
    assert!(out.contains("endpoint    greeter"), "{out}");
    assert!(out.contains("bindings    name=world"), "{out}");
}

#[test]
fn a_capability_refusal_names_every_missing_scope() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gated = counting("vault", &calls).with_description(
        Description::new("vault")
            .verb(Verb::Source)
            .requires("urn:cap:vault:read")
            .requires("urn:cap:vault:audit"),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:t:vault"), gated),
    ));
    let asker = Capability::scoped([INSPECT, "urn:cap:vault:audit"]);
    let out = explain_in(&kernel, "urn:t:vault", &[], &asker, Scope::empty()).unwrap();
    assert!(member_line(&out, "root").contains("answered"), "{out}");
    assert!(out.contains("endpoint    vault"), "{out}");
    assert!(
        out.contains("requires    urn:cap:vault:read, urn:cap:vault:audit"),
        "{out}"
    );
    assert!(
        out.contains("denied      yes: the capability lacks urn:cap:vault:read —"),
        "{out}"
    );
    // The explanation is the kernel's own verdict: the real request is Denied for
    // the scope it named, and the endpoint is never entered either way.
    let real = block_on(kernel.issue(Request::new(Verb::Source, iri("urn:t:vault")), &asker));
    assert!(
        matches!(&real, Err(Error::Denied(m)) if m.contains("urn:cap:vault:read")),
        "{real:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // Root lacks nothing.
    let out = explain(&kernel, "urn:t:vault", &[]);
    assert!(out.contains("denied      no"), "{out}");
}

#[test]
fn an_opaque_space_is_where_the_chain_stops_being_inspectable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hidden: Arc<dyn Space> = Arc::new(Opaque(greeter_root(&calls)));
    // As a corridor: it answers, and says that nothing inside it can be shown.
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let out = explain_in(
        &kernel,
        "urn:t:greet:alice",
        &[],
        &Capability::root(),
        Scope::empty().with_named(iri("urn:ctx:remote"), Arc::clone(&hidden)),
    )
    .unwrap();
    let line = member_line(&out, "urn:ctx:remote");
    assert!(
        line.contains("opaque") && line.contains("answered"),
        "{out}"
    );
    assert!(line.contains("cannot be shown"), "{out}");
    assert!(
        !out.contains("door "),
        "an opaque member names no door:\n{out}"
    );
    // Enclosed in a transparent root, the member says how many it encloses.
    let root = Fallback::new(vec![
        Arc::new(Mount::new("urn:remote:", Arc::clone(&hidden))) as Arc<dyn Space>,
        greeter_root(&calls),
    ]);
    let kernel = Kernel::new(Arc::new(root));
    let out = explain(&kernel, "urn:t:plain", &[]);
    assert!(
        member_line(&out, "root").contains("encloses 1 opaque space"),
        "{out}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_found_name_gives_the_endpoint_its_door_and_its_bindings() {
    let calls = Arc::new(AtomicUsize::new(0));
    let kernel = Kernel::new(greeter_root(&calls));
    let out = explain(&kernel, "urn:t:greet:alice", &[]);
    assert!(member_line(&out, "root").contains("answered"), "{out}");
    assert!(
        out.contains("endpoint    greeter (urn:ikigai:endpoint:greeter)"),
        "{out}"
    );
    assert!(
        out.contains("door        urn:t:greet:{name} (template)"),
        "{out}"
    );
    assert!(out.contains("bindings    name=alice"), "{out}");
    assert!(out.contains("requires    (none)"), "{out}");
    assert!(
        out.contains("cacheable   decided by the endpoint per answer"),
        "{out}"
    );
    // A mutating verb is never stored, and says so.
    let out = explain(&kernel, "urn:t:greet:alice", &[("verb", "sink")]);
    assert!(out.starts_with("explain sink urn:t:greet:alice"), "{out}");
    assert!(
        out.contains("cacheable   never: a sink is never stored"),
        "{out}"
    );
}

#[test]
fn a_shadowed_door_is_named_with_the_endpoint_that_would_have_answered() {
    let calls = Arc::new(AtomicUsize::new(0));
    let kernel = Kernel::new(greeter_root(&calls));
    let shadow = Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:t:plain"), counting("stand-in", &calls)),
    );
    let out = explain_in(
        &kernel,
        "urn:t:plain",
        &[],
        &Capability::root(),
        Scope::empty().with_named(iri("urn:ctx:shadow"), shadow),
    )
    .unwrap();
    assert!(
        member_line(&out, "urn:ctx:shadow").contains("answered"),
        "{out}"
    );
    let root = member_line(&out, "root");
    assert!(
        root.contains("shadowed") && root.contains("`plain`"),
        "{out}"
    );
    assert!(out.contains("endpoint    stand-in"), "{out}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn explain_invokes_nothing_is_cached_and_recomputes_on_a_binding_change() {
    let calls = Arc::new(AtomicUsize::new(0));
    let resolves = Arc::new(AtomicUsize::new(0));
    let kernel = Kernel::new(Arc::new(CountingSpace {
        inner: greeter_root(&calls),
        resolves: Arc::clone(&resolves),
    }));
    let first = explain(&kernel, "urn:t:greet:alice", &[]);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "a dry run invokes nothing");
    let walked = resolves.load(Ordering::SeqCst);
    assert!(walked > 0);
    // Cached: the second ask is served without walking the chain again.
    assert_eq!(explain(&kernel, "urn:t:greet:alice", &[]), first);
    assert_eq!(resolves.load(Ordering::SeqCst), walked);
    // A binding change cuts urn:kernel:bindings, which the explanation hangs from.
    kernel.bindings_changed();
    assert_eq!(explain(&kernel, "urn:t:greet:alice", &[]), first);
    assert!(
        resolves.load(Ordering::SeqCst) > walked,
        "recomputed after the cut"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn scopes_answer_for_a_narrower_capability_and_refuse_a_wider_one() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gated = counting("vault", &calls).with_description(
        Description::new("vault")
            .verb(Verb::Source)
            .requires("urn:cap:vault:read"),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:t:vault"), gated),
    ));
    let operator = Capability::scoped([INSPECT, "urn:cap:vault:read"]);
    // As the operator: admitted.
    let out = explain_in(&kernel, "urn:t:vault", &[], &operator, Scope::empty()).unwrap();
    assert!(out.contains("denied      no"), "{out}");
    // As a narrower grant inside the operator's: refused, and it says so.
    let out = explain_in(
        &kernel,
        "urn:t:vault",
        &[("scopes", INSPECT)],
        &operator,
        Scope::empty(),
    )
    .unwrap();
    assert!(
        out.contains(&format!("capability  attenuated to {INSPECT}")),
        "{out}"
    );
    assert!(
        out.contains("denied      yes: the capability lacks urn:cap:vault:read"),
        "{out}"
    );
    // Wider than the asker: refused outright, never silently narrowed.
    let wider = explain_in(
        &kernel,
        "urn:t:vault",
        &[("scopes", "urn:cap:vault:read urn:cap:vault:admin")],
        &operator,
        Scope::empty(),
    );
    assert!(
        matches!(&wider, Err(Error::Denied(m)) if m.contains("urn:cap:vault:admin")),
        "{wider:?}"
    );
}

#[test]
fn explain_needs_inspect_authority() {
    let calls = Arc::new(AtomicUsize::new(0));
    let kernel = Kernel::new(greeter_root(&calls));
    let refused = explain_in(
        &kernel,
        "urn:t:plain",
        &[],
        &Capability::scoped(Vec::<String>::new()),
        Scope::empty(),
    );
    assert!(
        matches!(&refused, Err(Error::Denied(m)) if m.contains(INSPECT)),
        "{refused:?}"
    );
}

#[test]
fn a_kernel_operation_is_explained_as_the_kernel_answering_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let kernel = Kernel::new(greeter_root(&calls));
    let out = explain_in(
        &kernel,
        "urn:kernel:cut",
        &[("verb", "sink")],
        &Capability::scoped([INSPECT]),
        Scope::empty(),
    )
    .unwrap();
    assert!(
        out.contains("the kernel itself, ahead of every space"),
        "{out}"
    );
    assert!(
        out.contains("denied      yes: the capability lacks urn:cap:kernel:cut"),
        "{out}"
    );
    let out = explain(&kernel, "urn:kernel:nonesuch", &[]);
    assert!(out.contains("serves no operation `nonesuch`"), "{out}");
}

#[test]
fn the_turtle_face_parses_and_joins_the_topology_and_the_catalog() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gated = counting("vault", &calls).with_description(
        Description::new("vault")
            .verb(Verb::Source)
            .requires("urn:cap:vault:read"),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:t:vault"), gated)
            .bind(
                UriTemplate::parse("urn:t:greet:{name}").unwrap(),
                counting("greeter", &calls),
            ),
    ));
    let turtle = explain_in(
        &kernel,
        "urn:t:vault",
        &[("as", "text/turtle")],
        &Capability::scoped([INSPECT]),
        Scope::empty(),
    )
    .unwrap();
    let triples: Vec<_> = oxttl::TurtleParser::new()
        .for_reader(turtle.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| panic!("not Turtle: {e}\n{turtle}"));
    let has = |predicate: &str, object: &str| {
        triples.iter().any(|t| {
            t.predicate.as_str() == format!("https://ikigai-rs.dev/ns#{predicate}")
                && t.object.to_string() == object
        })
    };
    assert!(has("answer", "<urn:ikigai:endpoint:vault>"), "{turtle}");
    assert!(has("lacks", "<urn:cap:vault:read>"), "{turtle}");
    assert!(has("chain", "<urn:ikigai:chain:root>"), "{turtle}");
    // The consultation points at the topology's own list cell for that member.
    assert!(has("layer", "<urn:ikigai:chain:root:layer:1>"), "{turtle}");
    assert!(
        has("outcome", "<urn:ikigai:explain:outcome:answered>"),
        "{turtle}"
    );
    assert!(!turtle.contains("_:"), "no blank nodes:\n{turtle}");
    // A binding is a skolemized node, not a blank one.
    let turtle = explain_in(
        &kernel,
        "urn:t:greet:bob",
        &[("as", "text/turtle")],
        &Capability::root(),
        Scope::empty(),
    )
    .unwrap();
    assert!(turtle.contains("ik:bindingValue \"bob\""), "{turtle}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
