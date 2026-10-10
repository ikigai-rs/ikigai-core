//! **An endpoint can hang its answer, failure included, from a thread it names**
//! (ledger #1079, from ikigai-script's version case, ledger #1076).
//!
//! A version atom `urn:demo:script:s:version:{digest}` answers `NotFound` until that
//! content is published, and its absence is the SCRIPT's state: a publish cuts
//! `urn:demo:script:s`, never the version IRI. A composite that catches the `NotFound`
//! and caches a fallback hung only from what the failure carried (the version IRI and
//! the atom's own sub-requests), so the publish did not reach it and the fallback was
//! served forever. `Invocation::depends_on` lets the atom name the thread its answer
//! really depends on, and a failure carries it, but only a miss (`NotFound`,
//! `Unresolved`) is ever cached on it: every other error class stays never-cached
//! whatever threads the endpoint attached.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, UriTemplate, Verb,
};

const SCRIPT: &str = "urn:demo:script:s";
const VERSION: &str = "urn:demo:script:s:version:abc";
const LATEST: &str = "urn:demo:latest";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// One error class: its kind, and how to make one.
type Case = (&'static str, fn() -> Error);

/// The published versions: the state a publish writes and cuts `SCRIPT` for.
type Published = Arc<Mutex<BTreeSet<String>>>;

/// A version atom over `published` and a composite over it that caches a fallback on
/// `NotFound`. `declares` says whether the atom hangs its answer from `SCRIPT`.
fn kernel(published: Published, declares: bool) -> Kernel {
    let atom = FnEndpoint::new("version", move |inv| {
        let digest = inv.bindings.get("digest").unwrap_or_default().to_string();
        if declares {
            inv.depends_on(SCRIPT);
        }
        if published.lock().unwrap().contains(&digest) {
            Ok(text(&digest).cacheable())
        } else {
            Err(Error::NotFound(format!(
                "version {digest} is not published"
            )))
        }
    });
    let latest = AsyncFnEndpoint::new("latest", |inv| {
        Box::pin(async move {
            match inv.source(&iri(VERSION)).await {
                Ok(found) => Ok(found.cacheable()),
                Err(Error::NotFound(_)) => Ok(text("fallback").cacheable()),
                Err(other) => Err(other),
            }
        })
    });
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                UriTemplate::parse("urn:demo:script:s:version:{digest}").unwrap(),
                atom,
            )
            .bind(Exact::new(LATEST), latest),
    ))
}

fn read(kernel: &Kernel) -> Vec<u8> {
    block_on(kernel.issue(source(LATEST), &Capability::root()))
        .unwrap()
        .bytes
}

/// What a publish does: write the state, cut the script's thread.
fn publish(kernel: &Kernel, published: &Published, digest: &str) {
    published.lock().unwrap().insert(digest.to_string());
    kernel.cut(SCRIPT);
}

/// The trap, reproduced: an atom that does not name the thread its absence depends on
/// leaves a fallback over it cached across the publish that ends the absence.
#[test]
fn a_fallback_over_an_undeclared_absence_survives_the_publish() {
    let published = Published::default();
    let kernel = kernel(published.clone(), false);
    assert_eq!(read(&kernel), b"fallback");
    assert!(kernel.is_cached(&source(LATEST), &Capability::root()));

    publish(&kernel, &published, "abc");
    assert!(
        kernel.is_cached(&source(LATEST), &Capability::root()),
        "nothing the publish cut reaches the fallback"
    );
    assert_eq!(read(&kernel), b"fallback", "stale: abc is published");
}

/// The fix: the atom names `SCRIPT`, the failure carries it, and the publish cuts the
/// fallback.
#[test]
fn a_fallback_over_a_declared_absence_is_cut_by_the_publish() {
    let published = Published::default();
    let kernel = kernel(published.clone(), true);
    assert_eq!(read(&kernel), b"fallback");
    assert!(kernel.is_cached(&source(LATEST), &Capability::root()));

    publish(&kernel, &published, "abc");
    assert!(
        !kernel.is_cached(&source(LATEST), &Capability::root()),
        "the publish cut the thread the atom's absence hung from"
    );
    assert_eq!(read(&kernel), b"abc");
}

/// On success it is the same edge as `Representation::depends_on`: the version's
/// answer, and the composite over it, are cut by the script's thread.
#[test]
fn on_success_it_is_an_ordinary_thread() {
    let published = Published::default();
    published.lock().unwrap().insert("abc".to_string());
    let kernel = kernel(published.clone(), true);
    assert_eq!(read(&kernel), b"abc");
    assert!(kernel.is_cached(&source(VERSION), &Capability::root()));
    assert!(kernel.is_cached(&source(LATEST), &Capability::root()));
    kernel.cut(SCRIPT);
    assert!(!kernel.is_cached(&source(VERSION), &Capability::root()));
    assert!(!kernel.is_cached(&source(LATEST), &Capability::root()));
}

/// An atom that attaches `SCRIPT` and then fails with `error`, under a composite that
/// catches ANY error and returns a cacheable fallback: what the kernel makes of it.
fn swallowed(error: fn() -> Error) -> Kernel {
    let atom = FnEndpoint::new("failing", move |inv| {
        inv.depends_on(SCRIPT);
        Err(error())
    });
    let latest = AsyncFnEndpoint::new("latest", |inv| {
        Box::pin(async move {
            match inv.source(&iri(VERSION)).await {
                Ok(found) => Ok(found.cacheable()),
                Err(_) => Ok(text("fallback").cacheable()),
            }
        })
    });
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new(VERSION), atom)
            .bind(Exact::new(LATEST), latest),
    ))
}

/// A miss propagated by an endpoint (`Unresolved` as well as `NotFound`) carries the
/// thread: cached, and cut by it.
#[test]
fn both_misses_are_cached_on_the_attached_thread() {
    let misses: [Case; 2] = [
        ("not-found", || Error::NotFound("absent".into())),
        ("unresolved", || {
            Error::Unresolved(Iri::parse("urn:demo:elsewhere").unwrap())
        }),
    ];
    for (kind, error) in misses {
        let kernel = swallowed(error);
        assert_eq!(read(&kernel), b"fallback", "{kind}");
        assert!(
            kernel.is_cached(&source(LATEST), &Capability::root()),
            "{kind}: a miss is a cacheable dependency"
        );
        kernel.cut(SCRIPT);
        assert!(
            !kernel.is_cached(&source(LATEST), &Capability::root()),
            "{kind}: the attached thread reaches the fallback"
        );
    }
}

/// ★ Every other error class stays never-cached, even with a thread attached: a
/// thread on a refusal would be a promise nobody keeps.
#[test]
fn every_other_error_stays_uncached_whatever_thread_is_attached() {
    let refusals: [Case; 8] = [
        ("denied", || Error::Denied("no".into())),
        ("timeout", || Error::Timeout("slow".into())),
        ("unavailable", || Error::Unavailable("down".into())),
        ("endpoint", || Error::Endpoint("broke".into())),
        ("conflict", || Error::Conflict("taken".into())),
        ("missing-argument", || Error::MissingArgument("x".into())),
        ("invalid-argument", || Error::InvalidArgument {
            name: "x".into(),
            detail: "bad".into(),
        }),
        ("depth-exceeded", || Error::DepthExceeded {
            depth: 64,
            target: Iri::parse(VERSION).unwrap(),
        }),
    ];
    for (kind, error) in refusals {
        let kernel = swallowed(error);
        assert_eq!(read(&kernel), b"fallback", "{kind}");
        assert!(
            !kernel.is_cached(&source(LATEST), &Capability::root()),
            "{kind}: a fallback over a refusal is never cached"
        );
    }
}

/// Calling it from a detached invocation (no kernel) is harmless.
#[test]
fn a_detached_invocation_accepts_it() {
    let request = source(VERSION);
    let bindings = ikigai_core::Bindings::default();
    let capability = Capability::root();
    let inv = ikigai_core::Invocation::detached(&request, &bindings, &capability);
    inv.depends_on(SCRIPT);
    inv.depends_on(SCRIPT);
}
