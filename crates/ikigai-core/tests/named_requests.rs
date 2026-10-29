//! **Named requests** (ledger #2512, K2 of the design accepted 2026-10-28): a root rule
//! that binds a name to a request WITH ARGUMENTS — a verb, fixed arguments (by value, by
//! reference, by capture), a template logical name and inputs of its own.
//!
//! A binding is partial application. What it fixes the caller may not give (refused,
//! naming the binding); every other argument passes through; `as` is never fixed, and a
//! binding's `as` is a declared input with a default, so a caller's `as=` still
//! negotiates. One test per acceptance criterion of the item, and one per refusal:
//!
//! 1. a table built from Turtle answers a bound name exactly as the expansion would;
//! 2. the declaration round trip is lossless;
//! 3. a cycle between two bindings in one table is refused at build, and one through two
//!    tables is `DepthExceeded`;
//! 4. a bound name's answer is its own cache entry, depending on the expansion's;
//! 5. a caller's fixed argument is refused, naming the binding;
//! 6. the line form refuses a rule with arguments, naming the forms that can write one.
#![cfg(feature = "declare")]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    build, Alias, AliasTable, ArgRef, ArgSpec, Body, Bytes, Capability, DeclarationError,
    Description, EndpointSpace, Error, Exact, Fallback, FixedArgument, FnEndpoint, Iri, Kernel,
    MatchKind, Registry, ReprType, Representation, Request, ResolveAt, Space, SpaceKind, Topology,
    TopologyRule, Verb,
};

const SPARQL: &str = "urn:example:sparql";
const COMPOSE: &str = "urn:example:compose";

/// The names of the design note's running example, as a declaration (the canonical form;
/// `named-requests-samples/quakes.names.ttl` is its proposal, with `ik:maxHops` added).
const NAMES: &str = r#"
@prefix ik: <https://ikigai-rs.dev/ns#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .

<urn:example:space:names> a ik:Alias ;
    ik:maxHops 8 ;
    ik:rewrites <urn:example:space:names:rule:1>, <urn:example:space:names:rule:2> ;
    ik:space <urn:example:space:names:empty> .
<urn:example:space:names:empty> a ik:EndpointSpace ; ik:doors rdf:nil .

<urn:example:space:names:rule:2> a ik:RewriteRule ;
    ik:ruleKind "exact" ;
    ik:logical "urn:example:quakes:count" ;
    ik:canonical "urn:example:sparql" ;
    ik:resolveAt "root" ;
    ik:verb "Source" ;
    ik:fixes <urn:example:space:names:rule:2:arg:graph>,
             <urn:example:space:names:rule:2:arg:query> ;
    ik:input <urn:example:space:names:rule:2:input:as> .
<urn:example:space:names:rule:2:arg:graph> a ik:Argument ;
    ik:inputName "graph" ; ik:value "urn:example:quakes:graph" .
<urn:example:space:names:rule:2:arg:query> a ik:Argument ;
    ik:inputName "query" ; ik:ref <urn:example:queries/count.rq> .
<urn:example:space:names:rule:2:input:as> ik:inputName "as" ; ik:required false ; ik:default "text/csv" .

<urn:example:space:names:rule:1> a ik:RewriteRule ;
    ik:ruleKind "template" ;
    ik:logical "urn:example:quakes:event:{id}" ;
    ik:canonical "urn:example:compose" ;
    ik:resolveAt "root" ;
    ik:verb "Source" ;
    ik:fixes <urn:example:space:names:rule:1:arg:src>, <urn:example:space:names:rule:1:arg:id> .
<urn:example:space:names:rule:1:arg:src> a ik:Argument ;
    ik:inputName "src" ; ik:value "urn:example:templates/event.html" .
<urn:example:space:names:rule:1:arg:id> a ik:Argument ; ik:inputName "id" ; ik:capture "id" .
"#;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// Every argument of the request, one `name=value` per line in name order; a reference
/// as `<iri>`. What both targets answer, so an answer says exactly what was asked.
fn echo(request: &Request) -> Vec<u8> {
    let mut out = String::new();
    for (name, value) in &request.args {
        let shown = match value {
            ArgRef::Inline(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            ArgRef::Reference(iri) => format!("<{}>", iri.as_str()),
            ArgRef::Content(id) => id.to_string(),
        };
        out.push_str(&format!("{name}={shown}\n"));
    }
    out.into_bytes()
}

/// A cacheable target that echoes its arguments and counts its runs.
fn echoing(name: &'static str, runs: Arc<AtomicU32>) -> FnEndpoint {
    FnEndpoint::new(name, move |inv| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(ReprType::new("text/plain"), echo(inv.request)).cacheable())
    })
    .with_description(Description::new(name).verb(Verb::Source))
}

struct Host {
    kernel: Kernel,
    runs: Arc<AtomicU32>,
}

/// The host's own doors, with the names declaration placed LAST in its `Fallback`.
fn host(names: &Topology) -> Host {
    let runs = Arc::new(AtomicU32::new(0));
    let doors = EndpointSpace::new()
        .bind(Exact::new(SPARQL), echoing("sparql", Arc::clone(&runs)))
        .bind(Exact::new(COMPOSE), echoing("compose", Arc::clone(&runs)));
    let names = build(names, &Registry::new()).expect("the names declaration builds");
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(doors), names]));
    Host {
        kernel: Kernel::new(root),
        runs,
    }
}

impl Host {
    fn issue(&self, request: Request) -> Result<String, Error> {
        block_on(self.kernel.issue(request, &Capability::root()))
            .map(|r| String::from_utf8(r.bytes).unwrap())
    }
}

fn names() -> Topology {
    Topology::from_turtle(NAMES).expect("the names declaration parses")
}

// 1 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_table_built_from_turtle_answers_a_bound_name_as_the_expansion_would() {
    let host = host(&names());
    let bound = host.issue(source("urn:example:quakes:count")).unwrap();
    let expansion = host
        .issue(
            source(SPARQL)
                .with_arg(
                    "graph",
                    ArgRef::Inline(b"urn:example:quakes:graph".to_vec()),
                )
                .with_arg(
                    "query",
                    ArgRef::Reference(iri("urn:example:queries/count.rq")),
                )
                .with_arg("as", ArgRef::Inline(b"text/csv".to_vec())),
        )
        .unwrap();
    assert_eq!(bound, expansion);
    assert_eq!(
        bound,
        "as=text/csv\ngraph=urn:example:quakes:graph\nquery=<urn:example:queries/count.rq>\n"
    );

    // A template rule: the WHOLE captured value is the argument, by value.
    let event = host
        .issue(source("urn:example:quakes:event:us7000abcd"))
        .unwrap();
    assert_eq!(
        event,
        "id=us7000abcd\nsrc=urn:example:templates/event.html\n"
    );
}

#[test]
fn every_unfixed_argument_passes_through_and_as_still_negotiates() {
    let host = host(&names());
    // A caller's `as` replaces the binding's default face: it is an input, never fixed.
    let turtle = host
        .issue(
            source("urn:example:quakes:count")
                .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec())),
        )
        .unwrap();
    assert!(turtle.starts_with("as=text/turtle\n"), "{turtle}");
    // An argument the binding says nothing about reaches the target unchanged.
    let limited = host
        .issue(source("urn:example:quakes:count").with_arg("limit", ArgRef::Inline(b"5".to_vec())))
        .unwrap();
    assert!(limited.contains("limit=5\n"), "{limited}");
}

#[test]
fn a_named_request_answers_its_verb_exists_and_meta_and_refuses_the_rest() {
    let host = host(&names());
    let sink = Request::new(Verb::Sink, iri("urn:example:quakes:count"))
        .with_arg("content", ArgRef::Inline(b"x".to_vec()));
    let refused = host.issue(sink).unwrap_err();
    assert!(
        matches!(
            refused,
            Error::Unsupported {
                verb: Some(Verb::Sink),
                ..
            }
        ),
        "{refused}"
    );
    assert!(
        refused.to_string().contains("<urn:example:quakes:count>"),
        "{refused}"
    );
}

// 2 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_declaration_round_trip_is_lossless() {
    let parsed = names();
    let SpaceKind::Alias { rules, .. } = &parsed.kind else {
        panic!("an alias node");
    };
    // Sorted most specific first: the longer logical name, the template, comes first.
    assert_eq!(rules[0].match_kind(), MatchKind::Template);
    assert_eq!(rules[1].verb, Some(Verb::Source));
    assert_eq!(rules[1].inputs[0].default.as_deref(), Some("text/csv"));

    let built = build(&parsed, &Registry::new()).unwrap();
    assert_eq!(
        built.topology(),
        parsed,
        "build keeps every part of every rule"
    );
    let again = Topology::from_turtle(&built.topology().to_turtle()).unwrap();
    assert_eq!(
        again, parsed,
        "and the rendering reads back as the same tree"
    );
}

#[test]
fn a_rule_built_in_code_renders_as_its_declaration() {
    let table = AliasTable::new()
        .with_rule(
            TopologyRule::named_request("urn:example:quakes:shout", "urn:example:plan:eval")
                .verb(Verb::Source)
                .fixing(FixedArgument::reference("in", iri("urn:example:shout.ttl")))
                .input(
                    ArgSpec::new("who")
                        .summary("who to shout at")
                        .default_value("world"),
                ),
        )
        .unwrap();
    let alias = Alias::new(Arc::new(table), Arc::new(EndpointSpace::new()))
        .named(iri("urn:example:space:shout"));
    let turtle = alias.topology().to_turtle();
    for expected in [
        "ik:fixes <urn:example:space:shout:rule:1:arg:in>",
        "ik:ref <urn:example:shout.ttl>",
        "ik:input <urn:example:space:shout:rule:1:input:who>",
        "ik:default \"world\"",
        "ik:verb \"Source\"",
    ] {
        assert!(turtle.contains(expected), "{expected} in\n{turtle}");
    }
    assert_eq!(Topology::from_turtle(&turtle).unwrap(), alias.topology());
}

// 3 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_cycle_between_two_bindings_in_one_table_is_refused_at_build() {
    let a = TopologyRule::named_request("urn:example:a", "urn:example:b")
        .fixing(FixedArgument::value("x", "1"));
    let b = TopologyRule::named_request("urn:example:b", "urn:example:a")
        .fixing(FixedArgument::value("y", "2"));
    let table = AliasTable::new()
        .with_rule(a)
        .unwrap()
        .with_rule(b)
        .unwrap();
    let alias = Alias::new(Arc::new(table), Arc::new(EndpointSpace::new()));
    let refused = build(&alias.topology(), &Registry::new())
        .err()
        .expect("refused");
    assert!(
        matches!(&refused, DeclarationError::Malformed { reason, .. } if reason.contains("alias cycle")),
        "{refused}"
    );
}

#[test]
fn a_cycle_through_two_tables_stops_at_the_depth_bound() {
    // A declared default is filled only when absent, so each pass through `a` adds
    // nothing a later pass would refuse: only the depth bound can stop the loop.
    let one = AliasTable::new()
        .with_rule(
            TopologyRule::named_request("urn:example:a", "urn:example:b")
                .input(ArgSpec::new("x").default_value("1")),
        )
        .unwrap();
    let two = AliasTable::new()
        .with_rule(TopologyRule::named_request(
            "urn:example:b",
            "urn:example:a",
        ))
        .unwrap();
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(Alias::new(Arc::new(one), Arc::new(EndpointSpace::new()))),
        Arc::new(Alias::new(Arc::new(two), Arc::new(EndpointSpace::new()))),
    ]));
    let kernel = Kernel::new(root);
    let err = block_on(kernel.issue(source("urn:example:a"), &Capability::root())).unwrap_err();
    assert!(matches!(err, Error::DepthExceeded { .. }), "{err}");
}

// 4 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_bound_answer_is_its_own_cache_entry_depending_on_the_expansions() {
    let host = host(&names());
    let first = host.issue(source("urn:example:quakes:count")).unwrap();
    assert_eq!(host.runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.issue(source("urn:example:quakes:count")).unwrap(),
        first
    );
    assert_eq!(
        host.runs.load(Ordering::SeqCst),
        1,
        "the bound name is served cached"
    );
    // The expansion's own entry is shared with anyone who asks for it directly.
    let direct = source(SPARQL)
        .with_arg(
            "graph",
            ArgRef::Inline(b"urn:example:quakes:graph".to_vec()),
        )
        .with_arg(
            "query",
            ArgRef::Reference(iri("urn:example:queries/count.rq")),
        )
        .with_arg("as", ArgRef::Inline(b"text/csv".to_vec()));
    host.issue(direct).unwrap();
    assert_eq!(
        host.runs.load(Ordering::SeqCst),
        1,
        "the expansion was computed once"
    );
    // Cutting the target's thread recomputes the bound name.
    host.kernel.cut(SPARQL);
    host.issue(source("urn:example:quakes:count")).unwrap();
    assert_eq!(host.runs.load(Ordering::SeqCst), 2);
}

// 5 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_callers_fixed_argument_is_refused_naming_the_binding() {
    let host = host(&names());
    let err = host
        .issue(
            source("urn:example:quakes:count")
                .with_arg("graph", ArgRef::Inline(b"urn:example:other".to_vec())),
        )
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid argument `graph`: fixed by <urn:example:quakes:count> to urn:example:quakes:graph"
    );
    let err = host
        .issue(source("urn:example:quakes:event:42").with_arg("id", ArgRef::Inline(b"43".to_vec())))
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid argument `id`: fixed by <urn:example:quakes:event:42> to 42"
    );
    assert_eq!(
        host.runs.load(Ordering::SeqCst),
        0,
        "a refusal never reaches the target"
    );
}

#[test]
fn a_rule_that_cannot_mean_one_thing_is_refused() {
    let named = || TopologyRule::named_request("urn:example:x", "urn:example:y");
    let refusals = [
        (
            named()
                .resolving_at(ResolveAt::Child)
                .fixing(FixedArgument::value("a", "1")),
            "resolves from the root",
        ),
        (
            named().fixing(FixedArgument::value("as", "text/csv")),
            "never fixed",
        ),
        (
            named()
                .fixing(FixedArgument::value("a", "1"))
                .fixing(FixedArgument::value("a", "2")),
            "twice",
        ),
        (
            named().fixing(FixedArgument::capture("a", "id")),
            "does not have",
        ),
        (named().verb(Verb::Meta), "Meta"),
        (
            named()
                .fixing(FixedArgument::value("a", "1"))
                .input(ArgSpec::new("a").optional()),
            "both fixes and declares",
        ),
        (named().input(ArgSpec::new("id").binding()), "binding"),
        (
            named().input(ArgSpec::new("q").example("x")),
            "more than a declaration states",
        ),
        (
            TopologyRule::named_template("urn:example:e:{id}", "urn:example:page:{other}"),
            "does not capture",
        ),
        (
            TopologyRule::named_template("urn:example:e", "urn:example:y"),
            "at least one",
        ),
    ];
    for (rule, why) in refusals {
        let refused = AliasTable::new().with_rule(rule).expect_err("refused");
        assert!(refused.to_string().contains(why), "{why}: {refused}");
    }
}

// 6 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_line_form_refuses_a_rule_with_arguments_naming_the_forms_that_can_write_one() {
    let refused =
        AliasTable::parse("exact urn:example:count urn:example:sparql root graph=urn:example:g")
            .unwrap_err();
    assert!(refused.detail.contains("Turtle"), "{refused}");
    assert!(refused.detail.contains("names file"), "{refused}");
    // A named request's readout says what it fixes, in words the parser refuses rather
    // than half-reads.
    let table = AliasTable::new()
        .with_rule(
            TopologyRule::named_request("urn:example:count", SPARQL)
                .fixing(FixedArgument::value("graph", "urn:example:g")),
        )
        .unwrap();
    let text = table.to_text();
    assert_eq!(
        text,
        "exact urn:example:count urn:example:sparql root graph=urn:example:g\n"
    );
    assert!(AliasTable::parse(&text).is_err());
    // A template rule without arguments has a line form, and it round-trips.
    let template = "template urn:example:e:{id} urn:example:page:{id} root\n";
    assert_eq!(AliasTable::parse(template).unwrap().to_text(), template);
    assert!(AliasTable::parse("template urn:example:e:{id} urn:example:page:{id}").is_err());
}

// Bodies ─────────────────────────────────────────────────────────────────────────

#[test]
fn a_streamed_answer_streams_through_the_view() {
    // An endpoint whose body face is a one-shot stream.
    struct Streaming;
    #[async_trait::async_trait]
    impl ikigai_core::Endpoint for Streaming {
        async fn invoke(
            &self,
            inv: &ikigai_core::Invocation<'_>,
        ) -> ikigai_core::Result<Representation> {
            let body = self.invoke_body(inv).await?;
            body.collect_within(inv.memory_budget().expect("minted"))
                .await
        }
        async fn invoke_body(
            &self,
            _inv: &ikigai_core::Invocation<'_>,
        ) -> ikigai_core::Result<Representation<Body>> {
            let chunks = (1..=3).map(|n| Ok(Bytes::from(format!("{n} "))));
            Ok(Representation::from_body(
                ReprType::new("text/plain"),
                Body::once(futures::stream::iter(chunks)),
            ))
        }
    }
    let table = AliasTable::new()
        .with_rule(TopologyRule::named_request(
            "urn:example:counted",
            "urn:example:count",
        ))
        .unwrap();
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:count"), Streaming)),
        Arc::new(Alias::new(Arc::new(table), Arc::new(EndpointSpace::new()))),
    ]));
    let kernel = Kernel::new(root);
    let body =
        block_on(kernel.issue_body(source("urn:example:counted"), &Capability::root())).unwrap();
    assert!(
        body.bytes.is_one_shot(),
        "the view handed the stream over uncollected"
    );
    let bytes = block_on(kernel.issue(source("urn:example:counted"), &Capability::root())).unwrap();
    assert_eq!(bytes.bytes, b"1 2 3 ");
}

// A kernel-held table ─────────────────────────────────────────────────────────────

#[test]
fn a_named_request_in_the_kernels_own_table_keeps_its_fixed_arguments() {
    let runs = Arc::new(AtomicU32::new(0));
    let doors = EndpointSpace::new().bind(Exact::new(SPARQL), echoing("sparql", runs));
    let table = AliasTable::new()
        .with_rule(
            TopologyRule::named_request("urn:example:quakes:count", SPARQL)
                .fixing(FixedArgument::value("graph", "urn:example:quakes:graph")),
        )
        .unwrap();
    let kernel = Kernel::new(Arc::new(doors)).with_aliases(Arc::new(table));
    let answer = block_on(kernel.issue(source("urn:example:quakes:count"), &Capability::root()))
        .map(|r| String::from_utf8(r.bytes).unwrap())
        .unwrap();
    assert_eq!(
        answer, "graph=urn:example:quakes:graph\n",
        "never the bare target"
    );
}
