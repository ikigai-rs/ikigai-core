//! `Confine` — run an endpoint inside a severed resolution chain.
//!
//! The endpoint-side face of [`Scope`]'s trapdoor: the wrapped endpoint is
//! invoked exactly as it would be, except that every sub-request it issues
//! resolves in `⟨what the host injected, S⟩` with **no root**. A sub-request
//! for anything the chain does not bind is [`Error::Unresolved`], never
//! [`Error::Denied`], and the root endpoint it would have reached is never
//! entered — there is no decision to misconfigure, only a name with nowhere to
//! go. See `docs/design/resolution-scope.md`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::describe::Description;
use crate::endpoint::{Endpoint, Invocation};
use crate::error::Result;
use crate::iri::Iri;
use crate::repr::Representation;
use crate::space::{Scope, Space};

/// Run `inner` confined to `space`: its sub-requests resolve only against the
/// corridors the host injected for the request and against `space`, and the
/// root is cut off. `name` is the corridor's identity for the cache — see
/// [`Scope`] on what naming claims: two confinements naming alike share cached
/// answers and must bind the same doors.
///
/// The description and the name are the inner endpoint's own: to a caller the
/// confined endpoint offers the same contract, and the confinement is a fact
/// about what it can reach while it runs, not about what it takes or returns.
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicU32, Ordering};
/// use futures::executor::block_on;
/// use ikigai_core::{
///     AsyncFnEndpoint, Capability, Confine, EndpointSpace, Error, Exact, FnEndpoint, Iri,
///     Kernel, ReprType, Representation, Request, Verb,
/// };
///
/// // A root with a secret in it, and an extractor that will try to read it.
/// let reached = Arc::new(AtomicU32::new(0));
/// let secret = {
///     let reached = Arc::clone(&reached);
///     FnEndpoint::new("secret", move |_| {
///         reached.fetch_add(1, Ordering::SeqCst);
///         Ok(Representation::new(ReprType::new("text/plain"), b"s3cr3t".to_vec()))
///     })
/// };
/// let extractor = AsyncFnEndpoint::new("extract", |inv| {
///     Box::pin(async move {
///         inv.source(&Iri::parse("urn:data:secret").unwrap()).await
///     })
/// });
/// // The extractor is confined to a corridor holding only the document.
/// let document = Arc::new(EndpointSpace::new().bind(
///     Exact::new("urn:doc:1"),
///     FnEndpoint::new("doc", |_| {
///         Ok(Representation::new(ReprType::new("text/plain"), b"the document".to_vec()))
///     }),
/// ));
/// let kernel = Kernel::new(Arc::new(
///     EndpointSpace::new()
///         .bind(Exact::new("urn:data:secret"), secret)
///         .bind_arc(
///             Exact::new("urn:extract"),
///             Arc::new(Confine::new(
///                 Iri::parse("urn:ctx:doc:1").unwrap(),
///                 document,
///                 Arc::new(extractor),
///             )),
///         ),
/// ));
///
/// // Even under ROOT authority: the secret is unresolvable from inside, and the
/// // secret endpoint was never entered. Not denied — nowhere to go.
/// let err = block_on(kernel.issue(
///     Request::new(Verb::Source, Iri::parse("urn:extract").unwrap()),
///     &Capability::root(),
/// ))
/// .unwrap_err();
/// assert!(matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:data:secret"));
/// assert_eq!(reached.load(Ordering::SeqCst), 0);
/// ```
pub struct Confine {
    name: Iri,
    space: Arc<dyn Space>,
    inner: Arc<dyn Endpoint>,
}

impl Confine {
    /// Confine `inner` to `space`, a corridor named `name`.
    pub fn new(name: Iri, space: Arc<dyn Space>, inner: Arc<dyn Endpoint>) -> Self {
        Confine { name, space, inner }
    }

    /// The chain `inner` will run in when invoked from `outer`'s chain — for a
    /// caller that wants to see the confinement before invoking.
    pub fn chain_from(&self, outer: &Scope) -> Scope {
        outer
            .clone()
            .confined(self.name.clone(), Arc::clone(&self.space))
    }
}

#[async_trait]
impl Endpoint for Confine {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let confined = inv.confine(self.name.clone(), Arc::clone(&self.space));
        self.inner.invoke(&confined).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> Description {
        self.inner.describe()
    }
}
