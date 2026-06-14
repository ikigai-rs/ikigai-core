//! End-to-end M1: resolve a request through composed spaces to an endpoint,
//! then invoke it.

use std::sync::Arc;

use ikigai_core::{
    builtins, ArgRef, Capability, EndpointSpace, Exact, Fallback, Iri, Mount, Representation,
    Request, Resolution, Rewrite, Scope, Space, UriTemplate, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// Resolve `req` against `space` and invoke the hit, returning the bytes.
fn run(space: &dyn Space, req: &Request) -> Option<Vec<u8>> {
    match space.resolve(req, &Scope::empty()) {
        Resolution::Hit(resolved) => {
            let cap = Capability::root();
            let inv = ikigai_core::Invocation {
                request: req,
                bindings: &resolved.bindings,
                capability: &cap,
            };
            let rep: Representation = resolved.endpoint.invoke(&inv).unwrap();
            Some(rep.bytes)
        }
        Resolution::Miss => None,
    }
}

fn functions() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:fn:toUpper"), builtins::to_upper())
        .bind(Exact::new("urn:fn:reverseList"), builtins::reverse_list())
        .bind(
            UriTemplate::parse("urn:fn:echo/{message}").unwrap(),
            builtins::echo(),
        )
}

#[test]
fn resolves_and_invokes_closure_endpoint() {
    let space = functions();
    let req = Request::new(Verb::Source, iri("urn:fn:toUpper"))
        .with_arg("in", ArgRef::Inline(b"hello".to_vec()));
    assert_eq!(run(&space, &req).as_deref(), Some(b"HELLO".as_slice()));
}

#[test]
fn reverse_list_reverses_items() {
    let space = functions();
    let req = Request::new(Verb::Source, iri("urn:fn:reverseList"))
        .with_arg("in", ArgRef::Inline(b"a\nb\nc".to_vec()));
    assert_eq!(run(&space, &req).as_deref(), Some(b"c\nb\na".as_slice()));
}

#[test]
fn grammar_bindings_flow_to_endpoint() {
    let space = functions();
    let req = Request::new(Verb::Source, iri("urn:fn:echo/world"));
    assert_eq!(run(&space, &req).as_deref(), Some(b"world".as_slice()));
}

#[test]
fn unresolved_target_misses() {
    let space = functions();
    let req = Request::new(Verb::Source, iri("urn:fn:nope"));
    assert!(run(&space, &req).is_none());
}

#[test]
fn mount_gates_by_prefix() {
    let mounted = Mount::new("urn:fn:", Arc::new(functions()));
    let hit = Request::new(Verb::Source, iri("urn:fn:toUpper"))
        .with_arg("in", ArgRef::Inline(b"x".to_vec()));
    let off = Request::new(Verb::Source, iri("urn:other:toUpper"))
        .with_arg("in", ArgRef::Inline(b"x".to_vec()));
    assert_eq!(run(&mounted, &hit).as_deref(), Some(b"X".as_slice()));
    assert!(run(&mounted, &off).is_none());
}

#[test]
fn fallback_tries_in_order() {
    let empty: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    let real: Arc<dyn Space> = Arc::new(functions());
    let chain = Fallback::new(vec![empty, real]);
    let req = Request::new(Verb::Source, iri("urn:fn:toUpper"))
        .with_arg("in", ArgRef::Inline(b"hi".to_vec()));
    assert_eq!(run(&chain, &req).as_deref(), Some(b"HI".as_slice()));
}

#[test]
fn rewrite_remaps_target_before_resolution() {
    let rewritten = Rewrite::new(Arc::new(functions()), |iri| {
        (iri.as_str() == "urn:alias:up").then(|| Iri::parse("urn:fn:toUpper").unwrap())
    });
    let req = Request::new(Verb::Source, iri("urn:alias:up"))
        .with_arg("in", ArgRef::Inline(b"hey".to_vec()));
    assert_eq!(run(&rewritten, &req).as_deref(), Some(b"HEY".as_slice()));
}
