//! Kernel operations must refuse a face they cannot serve (ledger #2143).
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Error, Iri, Kernel, MetaRenderer, ReprType,
    Representation, Request, Verb,
};

struct TurtleRenderer;
impl MetaRenderer for TurtleRenderer {
    fn render(&self, _: &Description, _: &ReprType) -> ikigai_core::Result<Representation> {
        Ok(Representation::new(ReprType::new("text/turtle"), vec![]))
    }
}

const SINGLE: &[(&str, Verb, &str)] = &[
    ("aliases", Verb::Source, "text/plain"),
    ("cached", Verb::Source, "text/plain"),
    ("uncached", Verb::Source, "text/plain"),
    ("cut", Verb::Sink, "text/plain"),
    ("catalog", Verb::Source, "text/turtle"),
    ("topology", Verb::Source, "text/turtle"),
    ("validate", Verb::Source, "text/turtle"),
];

fn request(op: &str, verb: Verb, face: Option<&str>) -> Request {
    let mut req = Request::new(verb, Iri::parse(format!("urn:kernel:{op}")).unwrap());
    for (key, value) in match op {
        "cached" | "explain" => vec![("target", "urn:test:missing")],
        "cut" | "dependents" => vec![("thread", "urn:test:thread")],
        "validate" => vec![("endpoint", "urn:kernel:aliases"), ("verb", "source")],
        _ => vec![],
    } {
        req = req.with_arg(key, ArgRef::Inline(value.as_bytes().to_vec()));
    }
    if let Some(face) = face {
        req = req.with_arg("as", ArgRef::Inline(face.as_bytes().to_vec()));
    }
    req
}

#[test]
fn single_face_operations_refuse_unsupported_as() {
    let kernel =
        Kernel::with_meta_renderer(Arc::new(EndpointSpace::new()), Arc::new(TurtleRenderer));
    for &(op, verb, face) in SINGLE {
        for bad in [
            "application/json",
            if face == "text/plain" {
                "text/turtle"
            } else {
                "text/plain"
            },
        ] {
            let result = block_on(kernel.issue(request(op, verb, Some(bad)), &Capability::root()));
            match result {
                Err(Error::InvalidArgument { name, detail }) => {
                    assert_eq!(name, "as", "{op}");
                    assert!(
                        detail.contains(bad) && detail.contains(&format!("urn:kernel:{op}")),
                        "{detail}"
                    );
                    assert!(detail.ends_with(&format!("({face})")), "{detail}");
                }
                other => panic!("{op} as={bad}: expected InvalidArgument, got {other:?}"),
            }
        }
    }
}

#[test]
fn single_face_operations_accept_the_default_and_explicit_face() {
    let kernel =
        Kernel::with_meta_renderer(Arc::new(EndpointSpace::new()), Arc::new(TurtleRenderer));
    for &(op, verb, face) in SINGLE {
        for face_arg in [
            None,
            Some(face),
            Some(if face == "text/plain" {
                " text/plain "
            } else {
                " text/turtle "
            }),
        ] {
            let repr =
                block_on(kernel.issue(request(op, verb, face_arg), &Capability::root())).unwrap();
            assert_eq!(repr.repr_type.media_type, face, "{op}");
        }
    }
}

#[test]
fn two_face_operations_refuse_unsupported_as() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    for op in [
        "actions",
        "cache",
        "constraint",
        "dependents",
        "explain",
        "scheduler",
        "threads",
    ] {
        let result = block_on(kernel.issue(
            request(op, Verb::Source, Some("application/json")),
            &Capability::root(),
        ));
        match result {
            Err(Error::InvalidArgument { name, detail }) => {
                assert_eq!(name, "as", "{op}");
                assert!(
                    detail.contains("text/plain") && detail.contains("text/turtle"),
                    "{op}: {detail}"
                );
            }
            other => panic!("{op}: expected InvalidArgument, got {other:?}"),
        }
    }
}

#[test]
fn refusing_a_cut_does_not_cut_the_thread() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let threads = || {
        block_on(kernel.issue(request("threads", Verb::Source, None), &Capability::root()))
            .unwrap()
            .bytes
    };
    let before = threads();
    assert!(matches!(
        block_on(kernel.issue(
            request("cut", Verb::Sink, Some("text/turtle")),
            &Capability::root()
        )),
        Err(Error::InvalidArgument { .. })
    ));
    assert_eq!(threads(), before);
    block_on(kernel.issue(
        request("cut", Verb::Sink, Some("text/plain")),
        &Capability::root(),
    ))
    .unwrap();
    assert_ne!(threads(), before);
}

#[test]
fn single_face_validation_preserves_capability_gates() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    for &(op, verb, _) in SINGLE {
        // validate is deliberately ungated; it validates the caller's own authority.
        if op == "validate" {
            continue;
        }
        assert!(
            matches!(
                block_on(kernel.issue(
                    request(op, verb, Some("application/json")),
                    &Capability::scoped(["urn:cap:unrelated"])
                )),
                Err(Error::Denied(_))
            ),
            "{op}"
        );
    }
}
