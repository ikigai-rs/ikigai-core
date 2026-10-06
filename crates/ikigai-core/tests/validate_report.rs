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
