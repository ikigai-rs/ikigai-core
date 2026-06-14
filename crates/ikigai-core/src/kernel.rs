//! The resolution kernel: it ties spaces, endpoints, and the representation
//! cache together.
//!
//! [`Kernel::issue`] is `async` so the kernel can be driven by any executor — a
//! multi-threaded thread pool on native (NetKernel-style scheduling), a
//! single-threaded executor in the browser — without `ikigai-core` depending on
//! a runtime. Resolution itself is synchronous (pure routing); the asynchronous
//! work lives in endpoint invocation.
//!
//! Caching in M3a is deliberately conservative: a result is cached only when the
//! verb is idempotent *and* the endpoint opted in via [`Expiry::Never`] (a pure
//! function of content-addressed inputs). Dependency-tracked expiry — for
//! results that read mutable state — arrives in M3b.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::capability::Capability;
use crate::endpoint::{Invocation, Issuer};
use crate::error::{Error, Result};
use crate::repr::{Expiry, Representation};
use crate::request::{Request, RequestId};
use crate::space::{Resolution, Scope, Space};

/// Resolves requests against a root space, invokes the resolved endpoint, and
/// caches cacheable representations by their content-addressed request id.
pub struct Kernel {
    root: Arc<dyn Space>,
    cache: Mutex<HashMap<RequestId, Representation>>,
}

impl Kernel {
    /// A kernel over the given root space.
    pub fn new(root: Arc<dyn Space>) -> Self {
        Kernel {
            root,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Issue a request: return a valid cached representation if one exists,
    /// otherwise resolve, invoke the endpoint, and cache the result if cacheable.
    pub async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation> {
        let id = request.id();
        let cacheable_verb = request.verb.is_cacheable();

        // Representation-cache lookup (idempotent verbs only). The guard is
        // dropped before any await.
        if cacheable_verb {
            let hit = self.cache.lock().expect("cache lock").get(&id).cloned();
            if let Some(cached) = hit {
                return Ok(cached);
            }
        }

        // Resolution is synchronous, pure routing.
        let resolved = match self.root.resolve(&request, &Scope::empty()) {
            Resolution::Hit(resolved) => resolved,
            Resolution::Miss => return Err(Error::Unresolved(request.target.clone())),
        };

        // Invocation is asynchronous.
        let invocation = Invocation::with_issuer(&request, &resolved.bindings, capability, self);
        let representation = resolved.endpoint.invoke(&invocation).await?;

        // Effective expiry propagates from the dependencies: a result is
        // cacheable only if it opted in AND every dependency it read is cacheable.
        let effective = if representation.expiry == Expiry::Never
            && invocation.dependency_expiry() == Expiry::Never
        {
            Expiry::Never
        } else {
            Expiry::Always
        };
        let representation = representation.with_expiry(effective);

        if cacheable_verb && effective == Expiry::Never {
            self.cache
                .lock()
                .expect("cache lock")
                .insert(id, representation.clone());
        }
        Ok(representation)
    }

    /// The number of representations currently cached (diagnostics/tests).
    pub fn cache_len(&self) -> usize {
        self.cache.lock().expect("cache lock").len()
    }
}

#[async_trait]
impl Issuer for Kernel {
    async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation> {
        // Delegate to the inherent method (which the context calls back into).
        Kernel::issue(self, request, capability).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arg::ArgRef;
    use crate::builtins;
    use crate::endpoint::{Endpoint, FnEndpoint, Invocation};
    use crate::grammar::Exact;
    use crate::iri::Iri;
    use crate::repr::ReprType;
    use crate::space::EndpointSpace;
    use crate::verb::Verb;
    use futures::executor::block_on;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn iri(s: &str) -> Iri {
        Iri::parse(s).unwrap()
    }

    #[test]
    fn resolves_invokes_and_caches() {
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:toUpper"))
                .with_arg("in", ArgRef::Inline(b"hi".to_vec()))
        };
        let a = block_on(kernel.issue(req(), &cap)).unwrap();
        let b = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(a.bytes, b"HI");
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(kernel.cache_len(), 1);
    }

    #[test]
    fn cacheable_endpoint_runs_only_once() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let counter = FnEndpoint::new("count", |_inv: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(ReprType::new("text/plain"), b"x".to_vec()).cacheable())
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:fn:count"), counter),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:fn:count"));
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "second issue should be a cache hit"
        );
    }

    #[test]
    fn uncacheable_endpoint_runs_every_time() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        // No `.cacheable()` -> default Expiry::Always -> never cached.
        let volatile = FnEndpoint::new("vol", |_inv: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"x".to_vec(),
            ))
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:fn:vol"), volatile),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:fn:vol"));
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "uncacheable result recomputes"
        );
    }

    #[test]
    fn unresolved_target_errors() {
        let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
        let cap = Capability::root();
        let err = block_on(kernel.issue(Request::new(Verb::Source, iri("urn:fn:nope")), &cap))
            .unwrap_err();
        assert!(matches!(err, Error::Unresolved(_)));
    }

    /// A composing endpoint: dereference the `src` by-reference argument and
    /// upper-case its content.
    struct UpcaseOf;

    #[async_trait::async_trait]
    impl Endpoint for UpcaseOf {
        async fn invoke(&self, cx: &Invocation<'_>) -> Result<Representation> {
            let src = match cx.request.args.get("src") {
                Some(ArgRef::Reference(iri)) => iri.clone(),
                _ => return Err(Error::MissingArgument("src".to_string())),
            };
            let upstream = cx.source(&src).await?;
            let upper = String::from_utf8_lossy(&upstream.bytes).to_uppercase();
            Ok(Representation::new(ReprType::new("text/plain"), upper.into_bytes()).cacheable())
        }
    }

    #[test]
    fn composes_over_a_referenced_resource_and_caches() {
        static GREETING_CALLS: AtomicU32 = AtomicU32::new(0);
        let greeting = FnEndpoint::new("greeting", |_cx: &Invocation<'_>| {
            GREETING_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(ReprType::new("text/plain"), b"hello".to_vec()).cacheable())
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:data:greeting"), greeting)
            .bind(Exact::new("urn:fn:upcaseOf"), UpcaseOf);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:data:greeting")))
        };
        let a = block_on(kernel.issue(req(), &cap)).unwrap();
        let b = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(a.bytes, b"HELLO");
        assert_eq!(a.bytes, b.bytes);
        // Both deps cacheable -> composed result cached -> greeting sourced once.
        assert_eq!(GREETING_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn volatile_dependency_forbids_caching_the_composer() {
        static CLOCK_CALLS: AtomicU32 = AtomicU32::new(0);
        // No `.cacheable()` -> volatile dependency.
        let clock = FnEndpoint::new("clock", |_cx: &Invocation<'_>| {
            CLOCK_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"tick".to_vec(),
            ))
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:data:clock"), clock)
            .bind(Exact::new("urn:fn:upcaseOf"), UpcaseOf);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:data:clock")))
        };
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        // Expiry propagates: volatile dep -> composer not cached -> clock sourced twice.
        assert_eq!(CLOCK_CALLS.load(Ordering::SeqCst), 2);
    }
}
