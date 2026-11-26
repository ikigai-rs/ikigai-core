//! Source negotiation on `as=` (ledger #2153, with #647 and #1465): what the kernel does
//! today, and a minimal proof of the design in `docs/design/source-negotiation.md`.
//!
//! The first test pins today's behavior, the claim the note starts from: a `Source`
//! answer is served in the type its endpoint answered, whatever `as=` asked for, even when
//! a transreptor chain to the asked type is mounted. Only `Meta` is transrepted.
//!
//! The rest prove the note's central argument with the public API alone, changing nothing
//! in the kernel: negotiation is a COMPOSITE over the answer and a plan, so a negotiator
//! written as an ordinary endpoint already gets the caching and the golden threads the
//! note asks of a kernel-driven one. The negotiator here is test code, not a proposal for
//! a public type.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, AsyncFnEndpoint, Capability, Description, EndpointSpace, Error, Exact,
    FnEndpoint, Iri, Kernel, ReprType, Representation, Request, Verb,
};

const CSV: &str = "text/csv";
const JSON: &str = "application/json";
const THREAD: &str = "urn:test:table:thread";

/// A text/csv resource that hangs from `THREAD` and counts its runs.
fn table(runs: Arc<AtomicUsize>) -> FnEndpoint {
    FnEndpoint::new("table", move |inv| {
        runs.fetch_add(1, Ordering::SeqCst);
        inv.depends_on(THREAD);
        Ok(Representation::new(ReprType::new(CSV), b"a,b\n1,2\n".to_vec()).cacheable())
    })
}

/// A csv → json transreptor: one row of two columns into one object, enough to see it ran.
fn csv_to_json() -> FnEndpoint {
    FnEndpoint::new("csv-json", |inv| {
        let csv = inv.inline_str("content")?;
        let text = csv;
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().unwrap_or_default().split(',').collect();
        let row: Vec<&str> = lines.next().unwrap_or_default().split(',').collect();
        let fields: Vec<String> = head
            .iter()
            .zip(row.iter())
            .map(|(k, v)| format!("\"{k}\":\"{v}\""))
            .collect();
        Ok(Representation::new(
            ReprType::new(JSON),
            format!("{{{}}}", fields.join(",")).into_bytes(),
        )
        .cacheable())
    })
    .with_description(
        Description::new("csv-json")
            .verb(Verb::Source)
            .input(ArgSpec::new("content"))
            .input(ArgSpec::new("as"))
            .transreptor([CSV], [JSON]),
    )
}

/// The negotiation the note proposes, as an ordinary composite: `Source` the `target` it
/// is handed, and when its answer is not the `as` asked for, plan a chain from the
/// answer's type and run it. No chain is a refusal that names both types and blames `as`.
fn negotiator() -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("negotiate", |inv| {
        Box::pin(async move {
            let target = Iri::parse(inv.inline_str("target")?)
                .map_err(|e| Error::Endpoint(format!("bad target: {e}")))?;
            let wanted = inv.inline_str("as")?.trim().to_string();
            let answer = inv.source(&target).await?;
            let served = answer.repr_type.media_type.clone();
            if served == wanted {
                return Ok(answer);
            }
            let plan =
                inv.select_transreptor(&served, &wanted)
                    .ok_or_else(|| Error::InvalidArgument {
                        name: "as".to_string(),
                        detail: format!(
                            "`{target}` answers `{served}`, and nothing converts it to `{wanted}`"
                        ),
                    })?;
            let converted = inv.transrept(answer.into_body(), &plan).await?;
            let bytes = inv.collect(converted.bytes).await?;
            Ok(Representation::new(converted.repr_type, bytes.to_vec()).cacheable())
        })
    })
}

fn kernel(runs: Arc<AtomicUsize>) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:test:table"), table(runs))
            .bind(Exact::new("urn:test:csv-json"), csv_to_json())
            .bind(Exact::new("urn:test:negotiate"), negotiator()),
    ))
}

fn read_table_as(face: &str) -> Request {
    Request::new(Verb::Source, Iri::parse("urn:test:table").unwrap())
        .with_arg("as", ArgRef::Inline(face.as_bytes().to_vec()))
}

fn negotiate(target: &str, face: &str) -> Request {
    Request::new(Verb::Source, Iri::parse("urn:test:negotiate").unwrap())
        .with_arg("target", ArgRef::Inline(target.as_bytes().to_vec()))
        .with_arg("as", ArgRef::Inline(face.as_bytes().to_vec()))
}

/// The claim, reproduced: a plan text/csv → application/json is mounted and selectable,
/// and a `Source` of the csv resource with `as=application/json` still answers text/csv.
#[test]
fn today_a_source_answer_ignores_as_even_with_a_plan_mounted() {
    let kernel = kernel(Arc::new(AtomicUsize::new(0)));
    let plan = kernel
        .select_transreptor(CSV, JSON)
        .expect("a plan is mounted");
    assert_eq!(plan[0].endpoint, "urn:test:csv-json");

    let answer = block_on(kernel.issue(read_table_as(JSON), &Capability::root())).unwrap();
    assert_eq!(
        answer.repr_type.media_type, CSV,
        "today the kernel does not negotiate a Source"
    );
    assert_eq!(answer.bytes, b"a,b\n1,2\n");
}

/// The proof: negotiation as a composite answers the asked type, is cached, and hangs
/// from the thread of the answer it converted, so a change of the csv drops the json.
#[test]
fn a_negotiated_answer_is_cached_and_hangs_from_the_answers_threads() {
    let runs = Arc::new(AtomicUsize::new(0));
    let kernel = kernel(runs.clone());
    let root = Capability::root();

    let json = block_on(kernel.issue(negotiate("urn:test:table", JSON), &root)).unwrap();
    assert_eq!(json.repr_type.media_type, JSON);
    assert_eq!(json.bytes, br#"{"a":"1","b":"2"}"#);
    assert!(kernel.is_cached(&negotiate("urn:test:table", JSON), &root));

    // Served again from the cache: the csv endpoint does not run again.
    block_on(kernel.issue(negotiate("urn:test:table", JSON), &root)).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    // The golden thread crosses the transreption: cutting the csv's thread drops the json.
    kernel.cut(THREAD);
    assert!(!kernel.is_cached(&negotiate("urn:test:table", JSON), &root));
    block_on(kernel.issue(negotiate("urn:test:table", JSON), &root)).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

/// No path is a refusal that blames `as` and names both types, never a substitution.
#[test]
fn no_path_is_an_invalid_as_naming_both_types() {
    let kernel = kernel(Arc::new(AtomicUsize::new(0)));
    let err = block_on(kernel.issue(
        negotiate("urn:test:table", "image/png"),
        &Capability::root(),
    ))
    .unwrap_err();
    match err {
        Error::InvalidArgument { name, detail } => {
            assert_eq!(name, "as");
            assert!(
                detail.contains(CSV) && detail.contains("image/png"),
                "{detail}"
            );
        }
        other => panic!("expected InvalidArgument(as), got {other:?}"),
    }
}

/// An endpoint that serves its faces itself and refuses an unknown `as` (ledger #2143)
/// refuses before any answer exists, so there is nothing to negotiate over: under the
/// note's rule (negotiate an ANSWER, never an error) the refusal stands.
#[test]
fn an_endpoint_that_refuses_its_as_keeps_its_refusal() {
    let kernel = kernel(Arc::new(AtomicUsize::new(0)));
    let direct = Request::new(Verb::Source, Iri::parse("urn:kernel:aliases").unwrap())
        .with_arg("as", ArgRef::Inline(JSON.as_bytes().to_vec()));
    let err = block_on(kernel.issue(direct, &Capability::root())).unwrap_err();
    assert!(
        matches!(err, Error::InvalidArgument { ref name, .. } if name == "as"),
        "{err:?}"
    );
}
