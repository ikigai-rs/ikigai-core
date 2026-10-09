//! **`urn:kernel:dependents` — what a cut would recompute** (ledger #907), and the
//! cache as a graph: the Turtle faces of `urn:kernel:cache` and `urn:kernel:threads`
//! that the cache visualizer (ledger #912) draws.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Capability, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel,
    ReprType, Representation, Request, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

/// `urn:t:data` (a cacheable leaf), `urn:t:view` (a cacheable composite reading
/// it), `urn:t:other` (cacheable, independent) and `urn:t:live` (uncacheable).
fn kernel() -> Kernel {
    let view = AsyncFnEndpoint::new("view", |inv| {
        Box::pin(async move {
            let data = inv.source(&Iri::parse("urn:t:data").unwrap()).await?;
            Ok(text(&format!("view of {}", String::from_utf8_lossy(&data.bytes))).cacheable())
        })
    });
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:t:data"),
                FnEndpoint::new("data", |_| Ok(text("data").cacheable())),
            )
            .bind(Exact::new("urn:t:view"), view)
            .bind(
                Exact::new("urn:t:other"),
                FnEndpoint::new("other", |_| Ok(text("other").cacheable())),
            )
            .bind(
                Exact::new("urn:t:live"),
                FnEndpoint::new("live", |_| Ok(text("live"))),
            ),
    ))
}

fn read(kernel: &Kernel, target: &str) {
    block_on(kernel.issue(Request::new(Verb::Source, iri(target)), &Capability::root())).unwrap();
}

fn ask(kernel: &Kernel, op: &str, args: &[(&str, &str)]) -> Result<String, Error> {
    let mut request = Request::new(Verb::Source, iri(&format!("urn:kernel:{op}")));
    for (name, value) in args {
        request = request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec()));
    }
    block_on(kernel.issue(request, &Capability::root()))
        .map(|r| String::from_utf8(r.bytes).unwrap())
}

fn dependents(kernel: &Kernel, thread: &str) -> String {
    ask(kernel, "dependents", &[("thread", thread)]).unwrap()
}

/// The targets listed as dependents (the entry lines).
fn listed(readout: &str) -> Vec<String> {
    readout
        .lines()
        .skip(2)
        .filter_map(|l| l.split_whitespace().next())
        .filter(|t| t.starts_with("urn:"))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_thread_names_the_cached_reads_hanging_from_it_and_a_cut_clears_them() {
    let kernel = kernel();
    read(&kernel, "urn:t:view");
    read(&kernel, "urn:t:other");
    read(&kernel, "urn:t:live");
    // The composite hangs from its dependency's thread, as does the dependency.
    let out = dependents(&kernel, "urn:t:data");
    assert!(
        out.starts_with("dependents of urn:t:data  (cut 0 times)"),
        "{out}"
    );
    assert_eq!(listed(&out), ["urn:t:data", "urn:t:view"], "{out}");
    assert!(out.contains("2 live cached entries hang from it"), "{out}");
    // After the cut, neither would be served: nothing left for a cut to recompute.
    kernel.cut("urn:t:data");
    let out = dependents(&kernel, "urn:t:data");
    assert!(
        out.starts_with("dependents of urn:t:data  (cut 1 time)"),
        "{out}"
    );
    assert!(listed(&out).is_empty(), "{out}");
    assert!(out.contains("a cut would recompute nothing"), "{out}");
    // The independent entry is untouched, and still a dependent of its own thread.
    assert_eq!(listed(&dependents(&kernel, "urn:t:other")), ["urn:t:other"]);
    // And recomputing puts them back.
    read(&kernel, "urn:t:view");
    assert_eq!(
        listed(&dependents(&kernel, "urn:t:data")),
        ["urn:t:data", "urn:t:view"]
    );
}

#[test]
fn asking_changes_nothing_and_is_never_cached() {
    let kernel = kernel();
    read(&kernel, "urn:t:data");
    kernel.cut("urn:t:data");
    let before = kernel.cache_len();
    // A stale entry stays resident: reading the dependents does not evict it…
    dependents(&kernel, "urn:t:data");
    assert_eq!(kernel.cache_len(), before);
    // …and the answer itself is live state, never stored.
    dependents(&kernel, "urn:t:data");
    assert_eq!(kernel.cache_len(), before);
}

#[test]
fn dependents_is_inspect_gated_and_needs_a_thread() {
    let kernel = kernel();
    let refused = block_on(
        kernel.issue(
            Request::new(Verb::Source, iri("urn:kernel:dependents"))
                .with_arg("thread", ArgRef::Inline(b"urn:t:data".to_vec())),
            &Capability::scoped(Vec::<String>::new()),
        ),
    );
    assert!(
        matches!(&refused, Err(Error::Denied(m)) if m.contains("urn:cap:kernel:inspect")),
        "{refused:?}"
    );
    let missing = ask(&kernel, "dependents", &[]);
    assert!(
        matches!(missing, Err(Error::MissingArgument(ref a)) if a == "thread"),
        "{missing:?}"
    );
}

/// Parse Turtle into (subject, predicate, object) strings, failing on bad syntax.
fn triples(turtle: &str) -> Vec<(String, String, String)> {
    oxttl::TurtleParser::new()
        .for_reader(turtle.as_bytes())
        .map(|t| {
            let t = t.unwrap_or_else(|e| panic!("not Turtle: {e}\n{turtle}"));
            (
                t.subject.to_string(),
                t.predicate.as_str().to_string(),
                t.object.to_string(),
            )
        })
        .collect()
}

const IK: &str = "https://ikigai-rs.dev/ns#";

#[test]
fn the_cache_graph_states_entries_hanging_from_their_threads() {
    let kernel = kernel();
    read(&kernel, "urn:t:view");
    kernel.cut("urn:t:other");
    let turtle = ask(&kernel, "cache", &[("as", "text/turtle")]).unwrap();
    let graph = triples(&turtle);
    assert!(!turtle.contains("_:"), "no blank nodes:\n{turtle}");
    let objects = |subject: &str, predicate: &str| -> Vec<String> {
        graph
            .iter()
            .filter(|(s, p, _)| s == subject && p == &format!("{IK}{predicate}"))
            .map(|(_, _, o)| o.clone())
            .collect()
    };
    let entry_of = |target: &str| -> String {
        graph
            .iter()
            .find(|(_, p, o)| p == &format!("{IK}resolvedFrom") && o == &format!("<{target}>"))
            .map(|(s, _, _)| s.clone())
            .unwrap_or_else(|| panic!("no entry for {target}:\n{turtle}"))
    };
    let view = entry_of("urn:t:view");
    assert!(view.starts_with("<urn:ikigai:cache:entry:"), "{view}");
    let mut hangs = objects(&view, "hangsFrom");
    hangs.sort();
    assert_eq!(
        hangs,
        [
            "<urn:ikigai:thread:urn:t:data>",
            "<urn:ikigai:thread:urn:t:view>"
        ]
    );
    assert_eq!(objects(&view, "chain"), ["<urn:ikigai:chain:root>"]);
    assert_eq!(objects(&view, "entryState"), ["\"live\""]);
    // Counts are typed as the vocabulary's range says, so a face and a graph built
    // from the vocabulary state TERM-equal literals.
    assert!(
        objects(&view, "sizeBytes")[0]
            .ends_with("^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>"),
        "{turtle}"
    );
    // Every thread an entry hangs from is a node, with its cut count.
    assert_eq!(
        objects("<urn:ikigai:thread:urn:t:data>", "threadName"),
        ["\"urn:t:data\""]
    );
    // The entry IRI is stable: the dependents face names the same node.
    let deps = triples(
        &ask(
            &kernel,
            "dependents",
            &[("thread", "urn:t:data"), ("as", "text/turtle")],
        )
        .unwrap(),
    );
    assert!(
        deps.iter()
            .any(|(s, p, _)| *s == view && p == &format!("{IK}resolvedFrom")),
        "{deps:?}"
    );
    // The threads face states each cut thread with its count.
    let threads = triples(&ask(&kernel, "threads", &[("as", "text/turtle")]).unwrap());
    assert!(threads
        .iter()
        .any(|(s, p, o)| s == "<urn:ikigai:thread:urn:t:other>"
            && p == &format!("{IK}cutCount")
            && o == "\"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>"));
}

#[test]
fn a_face_nobody_offers_is_refused() {
    let kernel = kernel();
    for op in ["cache", "threads"] {
        let refused = ask(&kernel, op, &[("as", "application/json")]);
        assert!(
            matches!(refused, Err(Error::InvalidArgument { .. })),
            "{op}: {refused:?}"
        );
    }
    let refused = ask(
        &kernel,
        "dependents",
        &[("thread", "x"), ("as", "application/json")],
    );
    assert!(
        matches!(refused, Err(Error::InvalidArgument { .. })),
        "{refused:?}"
    );
    // The text faces are unchanged by the new argument.
    assert!(ask(&kernel, "cache", &[]).unwrap().starts_with("cache\n"));
}
