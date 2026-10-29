//! **A forwarding name is described by its TARGET's contract** (ledger #2513, K3 of the
//! named-requests design; #2065, #2080).
//!
//! Before this, `Meta` of a view showed `alias-view`'s generic five-verb contract with no
//! inputs, the catalog never listed a view, and the manifold never offered one; an engine
//! routing by that empty contract sent `in=hello` to a view as a positional value and
//! answered `IN=HELLO`. Now one helper, applied by the probe every walk shares and by the
//! `Meta` arm, describes a forwarder by its target:
//!
//! 1. `Meta` (JSON) of a bound name carries the target's inputs, the fixed ones LISTED and
//!    marked fixed, its outputs and its `requires`;
//! 2. a plain view's `Meta` carries its target's inputs and outputs (#2080's regression):
//!    `in` is now a declared input an engine names, not a value it misroutes;
//! 3. `urn:kernel:catalog` lists the bound name once;
//! 4. `urn:kernel:actions` offers it only to a capability holding the target's `requires`,
//!    and a template name as a template with its captures as bindings;
//! 5. a view whose target resolves to nothing is described as the forwarder, saying so;
//! 6. the same holds for a view inside a `DynamicSpace`, and an attached body rides
//!    through a view to a target that reads it as a body, uncollected.
#![cfg(feature = "declare")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use futures::StreamExt;
use ikigai_core::{
    ActionQuery, Alias, AliasTable, ArgRef, ArgSpec, AsyncFnEndpoint, Body, BodyIssue, ByteStream,
    Bytes, Capability, Description, DynamicSpace, Endpoint, EndpointSpace, Exact, Fallback,
    FixedArgument, FnEndpoint, InputSource, Invocation, Iri, Kernel, MetaRenderer, Registry,
    ReprType, Representation, Request, Result, Space, TopologyRule, Verb,
};

const READ: &str = "urn:cap:example:sparql";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// JSON for `application/json`; for every other face one line naming the endpoint, so a
/// catalog's body counts the descriptions it rendered.
struct Renderer;

impl MetaRenderer for Renderer {
    fn render(&self, description: &Description, target: &ReprType) -> Result<Representation> {
        if target.media_type == "application/json" {
            return Ok(Representation::new(
                ReprType::new("application/json"),
                serde_json::to_vec(description).unwrap(),
            ));
        }
        Ok(Representation::new(
            ReprType::new("text/turtle"),
            format!(
                "<urn:ikigai:endpoint:{}> a <urn:t:Endpoint> .\n",
                description.id
            )
            .into_bytes(),
        ))
    }
}

fn upper() -> FnEndpoint {
    FnEndpoint::new("upper", |inv| {
        let text = inv.inline_str("in")?.to_uppercase();
        Ok(Representation::new(ReprType::new("text/plain"), text.into_bytes()).cacheable())
    })
    .with_description(
        Description::new("upper")
            .title("Upper case")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").summary("the text"))
            .output("text/plain"),
    )
}

fn sparql() -> FnEndpoint {
    FnEndpoint::new("sparql", |_| {
        Ok(Representation::new(
            ReprType::new("text/csv"),
            b"n\n3\n".to_vec(),
        ))
    })
    .with_description(
        Description::new("sparql")
            .verb(Verb::Source)
            .input(ArgSpec::new("graph").summary("the graph"))
            .input(ArgSpec::new("query").summary("the query"))
            .input(
                ArgSpec::new("as")
                    .optional()
                    .one_of(["text/csv", "text/turtle"]),
            )
            .output("text/csv")
            .output("text/turtle")
            .requires(READ),
    )
}

fn compose() -> FnEndpoint {
    FnEndpoint::new("compose", |_| {
        Ok(Representation::new(
            ReprType::new("text/html"),
            b"<p/>".to_vec(),
        ))
    })
    .with_description(
        Description::new("compose")
            .verb(Verb::Source)
            .input(ArgSpec::new("src").class("http://www.w3.org/2001/XMLSchema#anyURI"))
            .open_arguments("each fills a {name} marker")
            .output("text/html"),
    )
}

fn names() -> AliasTable {
    AliasTable::new()
        .exact_at_root("urn:example:shout", "urn:example:upper")
        .exact_at_root("urn:example:nowhere", "urn:example:missing")
        .with_rule(
            TopologyRule::named_request("urn:example:count", "urn:example:sparql")
                .verb(Verb::Source)
                .fixing(FixedArgument::value("graph", "urn:example:graph"))
                .input(ArgSpec::new("as").default_value("text/csv")),
        )
        .unwrap()
        .with_rule(
            TopologyRule::named_template("urn:example:event:{id}", "urn:example:compose")
                .verb(Verb::Source)
                .fixing(FixedArgument::value("src", "urn:example:event.html"))
                .fixing(FixedArgument::capture("id", "id")),
        )
        .unwrap()
}

fn kernel() -> Kernel {
    let doors = EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), upper())
        .bind(Exact::new("urn:example:sparql"), sparql())
        .bind(Exact::new("urn:example:compose"), compose());
    let views = Alias::new(Arc::new(names()), Arc::new(EndpointSpace::new()));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(doors), Arc::new(views)]));
    Kernel::with_meta_renderer(root, Arc::new(Renderer))
}

/// `Meta as=application/json` of `name`, read back.
fn meta(kernel: &Kernel, name: &str) -> Description {
    let request = Request::new(Verb::Meta, iri(name))
        .with_arg("as", ArgRef::Inline(b"application/json".to_vec()));
    let answer = block_on(kernel.issue(request, &Capability::root())).unwrap();
    serde_json::from_slice(&answer.bytes).unwrap()
}

fn input<'a>(description: &'a Description, verb: Verb, name: &str) -> Option<&'a ArgSpec> {
    description
        .actions
        .iter()
        .find(|a| a.verb == verb)
        .and_then(|a| a.inputs.iter().find(|i| i.name == name))
}

// 1 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn meta_of_a_bound_name_is_its_targets_contract_with_the_fixed_inputs_marked() {
    let kernel = kernel();
    let count = meta(&kernel, "urn:example:count");
    assert_eq!(count.id, "urn:example:count");
    assert_eq!(count.verbs, [Verb::Source, Verb::Exists, Verb::Meta]);
    let graph = input(&count, Verb::Source, "graph").expect("a fixed input is LISTED");
    assert_eq!(graph.fixed.as_deref(), Some("urn:example:graph"));
    assert!(!graph.required);
    let query = input(&count, Verb::Source, "query").expect("an unfixed input passes through");
    assert!(query.required && query.fixed.is_none());
    // The binding's declared `as` replaces the target's: the name's default face.
    let face = input(&count, Verb::Source, "as").unwrap();
    assert_eq!(face.default.as_deref(), Some("text/csv"));
    let source = count
        .actions
        .iter()
        .find(|a| a.verb == Verb::Source)
        .unwrap();
    assert_eq!(source.outputs, ["text/csv", "text/turtle"]);
    assert_eq!(source.requires, [READ], "the target's requires, projected");
    let exists = count
        .actions
        .iter()
        .find(|a| a.verb == Verb::Exists)
        .unwrap();
    assert_eq!(
        exists.requires,
        [READ],
        "an undeclared Exists needs what Source needs"
    );
    // `Kernel::describe` is the same walk, through the shared probe.
    assert_eq!(kernel.describe(&iri("urn:example:count")).unwrap(), count);
}

// 2 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_plain_views_meta_carries_its_targets_inputs_and_outputs() {
    let kernel = kernel();
    let shout = meta(&kernel, "urn:example:shout");
    assert_eq!(shout.id, "urn:example:shout");
    assert_eq!(shout.title, "Upper case");
    assert_eq!(shout.outputs, ["text/plain"]);
    let input = input(&shout, Verb::Source, "in").expect("the target's `in`");
    // Required and declared: an engine routes `in=hello` to it by name.
    assert!(input.required && input.source == InputSource::Argument);
    // And the view answers as its target does.
    let request = Request::new(Verb::Source, iri("urn:example:shout"))
        .with_arg("in", ArgRef::Inline(b"hello".to_vec()));
    let answer = block_on(kernel.issue(request, &Capability::root())).unwrap();
    assert_eq!(answer.bytes, b"HELLO");
}

// 3 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_catalog_lists_the_bound_name_once() {
    let kernel = kernel();
    let catalog = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:kernel:catalog")),
        &Capability::root(),
    ))
    .unwrap();
    let body = String::from_utf8(catalog.bytes).unwrap();
    for name in [
        "urn:example:count",
        "urn:example:shout",
        "urn:example:event:{id}",
    ] {
        let line = format!("<urn:ikigai:endpoint:{name}> ");
        assert_eq!(body.matches(&line).count(), 1, "{name} once in\n{body}");
    }
    assert_eq!(
        body.matches("<urn:ikigai:endpoint:sparql> ").count(),
        1,
        "{body}"
    );
}

// 4 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_manifold_offers_a_bound_name_only_to_a_capability_holding_its_targets_requires() {
    let kernel = kernel();
    let offered = |capability: &Capability| -> Vec<String> {
        kernel
            .select_actions(&ActionQuery {
                capability: Some(capability),
                ..Default::default()
            })
            .into_iter()
            .map(|m| format!("{} {:?}", m.endpoint, m.verb))
            .collect()
    };
    let reader = offered(&Capability::scoped([READ]));
    assert!(
        reader.contains(&"urn:example:count Source".to_string()),
        "{reader:?}"
    );
    let stranger = offered(&Capability::scoped(["urn:cap:example:other"]));
    assert!(
        !stranger.iter().any(|m| m.starts_with("urn:example:count ")),
        "{stranger:?}"
    );
    // A template name is offered as a template, its capture a binding.
    assert!(
        stranger.contains(&"urn:example:event:{id} Source".to_string()),
        "{stranger:?}"
    );
    let event = kernel.describe(&iri("urn:example:event:42")).unwrap();
    let id = input(&event, Verb::Source, "id").unwrap();
    assert_eq!(id.source, InputSource::Binding);
    assert_eq!(
        input(&event, Verb::Source, "src").unwrap().fixed.as_deref(),
        Some("urn:example:event.html")
    );
}

// 5 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_view_onto_nothing_is_described_as_the_forwarder_saying_so() {
    let kernel = kernel();
    let nowhere = meta(&kernel, "urn:example:nowhere");
    assert_eq!(nowhere.id, "alias-view");
    assert!(
        nowhere
            .summary
            .contains("<urn:example:missing> resolves to nothing"),
        "{}",
        nowhere.summary
    );
}

// 6 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_view_in_a_dynamic_space_is_described_by_its_target_too() {
    let dynamic = Arc::new(DynamicSpace::new(
        "names",
        iri("urn:example:decl:names"),
        Registry::new(),
    ));
    let doors = EndpointSpace::new().bind(Exact::new("urn:example:upper"), upper());
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(doors), dynamic.clone()]));
    let kernel = Kernel::with_meta_renderer(root, Arc::new(Renderer));
    let views = Alias::new(
        Arc::new(AliasTable::new().exact_at_root("urn:example:shout", "urn:example:upper")),
        Arc::new(EndpointSpace::new()),
    );
    block_on(dynamic.install(&kernel, &views.topology().to_turtle())).unwrap();
    let shout = meta(&kernel, "urn:example:shout");
    assert!(input(&shout, Verb::Source, "in").is_some(), "{shout:?}");
}

type Log = Arc<Mutex<Vec<String>>>;

/// A one-shot body of five chunks, logging `p{i}` as each is produced.
fn counting(log: &Log) -> Body {
    let log = Arc::clone(log);
    Body::once(futures::stream::iter(0..5).map(move |i| {
        log.lock().unwrap().push(format!("p{i}"));
        Ok(Bytes::from(format!("{i}")))
    }))
}

struct Producer(Log);

#[async_trait]
impl Endpoint for Producer {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let body = self.invoke_body(inv).await?;
        body.collect_within(inv.memory_budget().unwrap()).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        Ok(Representation::from_body(
            ReprType::new("text/plain"),
            counting(&self.0),
        ))
    }
}

#[test]
fn an_attached_body_rides_through_a_view_to_a_target_that_reads_it_as_a_body() {
    let log: Log = Log::default();
    let stage_log = Arc::clone(&log);
    let join = AsyncFnEndpoint::new("join", move |inv| {
        let log = Arc::clone(&stage_log);
        Box::pin(async move {
            let mut stream: ByteStream = inv.content_body("content").await?.into_stream();
            let mut out = Vec::new();
            let mut n = 0;
            while let Some(chunk) = stream.next().await {
                log.lock().unwrap().push(format!("c{n}"));
                n += 1;
                out.extend_from_slice(&chunk?);
            }
            Ok(Representation::new(ReprType::new("text/plain"), out))
        })
    })
    .with_description(
        Description::new("join")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").reads_body())
            .output("text/plain"),
    );
    let doors = EndpointSpace::new()
        .bind(Exact::new("urn:example:count"), Producer(Arc::clone(&log)))
        .bind(Exact::new("urn:example:join"), join);
    let views = Alias::new(
        Arc::new(AliasTable::new().exact_at_root("urn:example:joined", "urn:example:join")),
        Arc::new(EndpointSpace::new()),
    );
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(doors), Arc::new(views)]));
    let kernel = Kernel::new(root);
    let root = Capability::root();
    let upstream =
        block_on(kernel.issue_body(Request::new(Verb::Source, iri("urn:example:count")), &root))
            .unwrap();
    let issue = BodyIssue::new().attach("content", upstream.bytes);
    let answer = block_on(kernel.issue_with(
        Request::new(Verb::Source, iri("urn:example:joined")),
        &root,
        issue,
    ))
    .unwrap();
    assert_eq!(answer.bytes, b"01234");
    let log = log.lock().unwrap();
    let at = |entry: &str| log.iter().position(|e| e == entry).unwrap();
    assert!(
        at("c0") < at("p4"),
        "the target read its first chunk before the producer ended: nothing was collected at \
         the view ({log:?})"
    );
}
