//! **`urn:kernel:validate`'s report is Turtle** (ledger #750, C2). The SHACL report
//! quotes caller input (an argument name or value) in `sh:resultMessage`, and only `\`
//! and `"` were escaped; `args` splits on `&` and LF, so a CR survived into a
//! `"…"`-quoted string, which Turtle forbids, and the `text/turtle` answer did not parse.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel,
    ReprType, Representation, Request, Verb,
};

fn report(args: &[u8]) -> String {
    let endpoint = FnEndpoint::new("ep", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"x".to_vec(),
        ))
    })
    .with_description(
        Description::new("ep")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").optional()),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:x:ep"), endpoint),
    ));
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:validate").unwrap())
        .with_arg("endpoint", ArgRef::Inline(b"urn:x:ep".to_vec()))
        .with_arg("verb", ArgRef::Inline(b"source".to_vec()))
        .with_arg("args", ArgRef::Inline(args.to_vec()));
    let report = block_on(kernel.issue(request, &Capability::root())).unwrap();
    assert_eq!(report.repr_type.media_type, "text/turtle");
    String::from_utf8(report.bytes).unwrap()
}

#[test]
fn a_report_quoting_any_character_still_parses_as_turtle() {
    for args in [
        &b"bad\rname=1"[..],
        b"bad\"name\\=1",
        b"bad\tname=1",
        b"in=1&bad\r\rname=\"x\"",
    ] {
        let text = report(args);
        assert!(
            text.contains("sh:resultMessage"),
            "no violation reported: {text}"
        );
        let parsed: Result<Vec<_>, _> = oxttl::TurtleParser::new()
            .for_reader(text.as_bytes())
            .collect();
        assert!(
            parsed.is_ok(),
            "validate returned unparseable Turtle for {args:?}: {parsed:?}\n{text:?}"
        );
    }
}

/// Ledger #1352: `sh:resultPath` names the contract's input node, spelled as the catalog
/// spells it (`ikigai-vocab`'s `to_turtle`: a `%` in an input NAME is `%25`). Left alone,
/// `100%` made the report unparseable, and `a%41` parsed but named a node the catalog does
/// not have (`…:input:a%41`, where the catalog says `…:input:a%2541`).
#[test]
fn a_result_path_for_an_input_name_with_a_percent_parses_and_joins_the_catalog() {
    let description = Description::new("ep")
        .verb(Verb::Source)
        .input(ArgSpec::new("100% sure"))
        .input(ArgSpec::new("a%41"));
    let contract = description
        .action_specs()
        .into_iter()
        .find(|a| a.verb == Verb::Source)
        .unwrap()
        .contract_iri("ep");
    let endpoint = FnEndpoint::new("ep", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"x".to_vec(),
        ))
    })
    .with_description(description);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:x:ep"), endpoint),
    ));
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:validate").unwrap())
        .with_arg("endpoint", ArgRef::Inline(b"urn:x:ep".to_vec()))
        .with_arg("verb", ArgRef::Inline(b"source".to_vec()))
        .with_arg("args", ArgRef::Inline(Vec::new()));
    let text = String::from_utf8(
        block_on(kernel.issue(request, &Capability::root()))
            .unwrap()
            .bytes,
    )
    .unwrap();
    let triples: Vec<_> = oxttl::TurtleParser::new()
        .for_reader(text.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| panic!("not Turtle: {e}\n{text}"));
    let mut paths: Vec<String> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == "http://www.w3.org/ns/shacl#resultPath")
        .map(|t| t.object.to_string())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            format!("<{contract}:input:100%25%20sure>"),
            format!("<{contract}:input:a%2541>"),
        ]
    );
}

/// Ledger #1361: an input name holding `[`, `]` or a code point outside RFC 3987's
/// `ucschar` is encoded in `sh:resultPath` exactly as the catalog encodes the input node
/// (`encode_name`), so the report parses and still joins the contract. Before, the path was
/// written with the raw character and the whole report was refused.
#[test]
fn a_result_path_for_an_input_name_with_brackets_or_a_non_ucschar_parses() {
    let description = Description::new("http://[::1]/ep")
        .verb(Verb::Source)
        .input(ArgSpec::new("a[0]"))
        .input(ArgSpec::new("nel\u{85}pua\u{E000}"));
    let contract = description
        .action_specs()
        .into_iter()
        .find(|a| a.verb == Verb::Source)
        .unwrap()
        .contract_iri(&description.id);
    let endpoint = FnEndpoint::new("ep", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"x".to_vec(),
        ))
    })
    .with_description(description);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:x:ep"), endpoint),
    ));
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:validate").unwrap())
        .with_arg("endpoint", ArgRef::Inline(b"urn:x:ep".to_vec()))
        .with_arg("verb", ArgRef::Inline(b"source".to_vec()))
        .with_arg("args", ArgRef::Inline(Vec::new()));
    let text = String::from_utf8(
        block_on(kernel.issue(request, &Capability::root()))
            .unwrap()
            .bytes,
    )
    .unwrap();
    let triples: Vec<_> = oxttl::TurtleParser::new()
        .for_reader(text.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| panic!("not Turtle: {e}\n{text}"));
    let mut paths: Vec<String> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == "http://www.w3.org/ns/shacl#resultPath")
        .map(|t| t.object.to_string())
        .collect();
    paths.sort();
    assert!(
        contract.starts_with("urn:ikigai:contract:http://%5B::1%5D/ep:source:b3:"),
        "{contract}"
    );
    assert_eq!(
        paths,
        [
            format!("<{contract}:input:a%5B0%5D>"),
            format!("<{contract}:input:nel%C2%85pua%EE%80%80>"),
        ]
    );
}
