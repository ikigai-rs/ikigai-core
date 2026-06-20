use std::collections::BTreeSet;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::arg::ArgRef;
use crate::capability::Capability;
use crate::describe::Description;
use crate::error::{Error, Result};
use crate::grammar::Bindings;
use crate::iri::Iri;
use crate::repr::{Expiry, Representation, Thread};
use crate::request::Request;
use crate::verb::Verb;

/// Lets an endpoint issue sub-requests back through the kernel. Implemented by
/// the [`Kernel`](crate::Kernel); a detached [`Invocation`] has no issuer, so
/// `source`/`issue` are unavailable when testing an endpoint in isolation.
#[async_trait]
pub trait Issuer: Send + Sync {
    /// Resolve and evaluate a sub-request.
    async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation>;
}

/// The context handed to an endpoint when it is invoked.
///
/// All input arrives here — the request, the grammar-captured bindings, and the
/// authorizing capability — and, when the kernel is driving, the ability to
/// issue sub-requests via [`Invocation::source`] / [`Invocation::issue`].
/// Endpoints take no ambient authority.
pub struct Invocation<'a> {
    /// The request being served.
    pub request: &'a Request,
    /// Variables captured by the grammar that resolved this request.
    pub bindings: &'a Bindings,
    /// The capability authorizing invocation.
    pub capability: &'a Capability,
    issuer: Option<&'a dyn Issuer>,
    deps: Mutex<Vec<Expiry>>,
    /// Union of the golden threads of every sub-resource resolved during this
    /// invocation — so the kernel can propagate them onto the result.
    dep_threads: Mutex<BTreeSet<Thread>>,
}

impl<'a> Invocation<'a> {
    /// A context with no kernel attached: `source`/`issue` are unavailable.
    /// Useful for invoking an endpoint directly in tests.
    pub fn detached(
        request: &'a Request,
        bindings: &'a Bindings,
        capability: &'a Capability,
    ) -> Self {
        Invocation {
            request,
            bindings,
            capability,
            issuer: None,
            deps: Mutex::new(Vec::new()),
            dep_threads: Mutex::new(BTreeSet::new()),
        }
    }

    /// A context backed by an issuer (the kernel), enabling sub-requests.
    pub(crate) fn with_issuer(
        request: &'a Request,
        bindings: &'a Bindings,
        capability: &'a Capability,
        issuer: &'a dyn Issuer,
    ) -> Self {
        Invocation {
            request,
            bindings,
            capability,
            issuer: Some(issuer),
            deps: Mutex::new(Vec::new()),
            dep_threads: Mutex::new(BTreeSet::new()),
        }
    }

    /// The bytes of an inline argument, or an error if absent / not inline.
    pub fn inline_arg(&self, name: &str) -> Result<&[u8]> {
        match self.request.args.get(name) {
            Some(ArgRef::Inline(bytes)) => Ok(bytes),
            Some(_) => Err(Error::InvalidArgument {
                name: name.to_string(),
                detail: "expected an inline value".to_string(),
            }),
            None => Err(Error::MissingArgument(name.to_string())),
        }
    }

    /// An inline argument decoded as UTF-8.
    pub fn inline_str(&self, name: &str) -> Result<&str> {
        std::str::from_utf8(self.inline_arg(name)?).map_err(|_| Error::InvalidArgument {
            name: name.to_string(),
            detail: "not valid UTF-8".to_string(),
        })
    }

    /// Issue a sub-request through the kernel, recording it as a dependency of
    /// this invocation's result so expiry propagates. Errors if detached.
    pub async fn issue(&self, request: Request) -> Result<Representation> {
        let issuer = self
            .issuer
            .ok_or_else(|| Error::Endpoint("sub-requests require a kernel context".to_string()))?;
        let representation = issuer.issue(request, self.capability).await?;
        self.deps
            .lock()
            .expect("deps lock")
            .push(representation.expiry);
        // Inherit the sub-resource's golden threads so cutting any of them
        // invalidates this (composite) result too.
        self.dep_threads
            .lock()
            .expect("dep threads lock")
            .extend(representation.threads().iter().cloned());
        Ok(representation)
    }

    /// `SOURCE` another resource — dereference a by-reference argument — recording
    /// it as a dependency.
    pub async fn source(&self, target: &Iri) -> Result<Representation> {
        self.issue(Request::new(Verb::Source, target.clone())).await
    }

    /// Combined expiry of the dependencies issued during this invocation:
    /// `Always` if any is volatile, else `Never`.
    pub(crate) fn dependency_expiry(&self) -> Expiry {
        let deps = self.deps.lock().expect("deps lock");
        if deps.contains(&Expiry::Always) {
            Expiry::Always
        } else {
            Expiry::Never
        }
    }

    /// The union of golden threads of every dependency resolved during this
    /// invocation — the kernel unions these onto the result's own threads.
    pub(crate) fn dependency_threads(&self) -> BTreeSet<Thread> {
        self.dep_threads.lock().expect("dep threads lock").clone()
    }
}

/// An endpoint produces a [`Representation`] in response to a request.
///
/// Endpoints are synchronous and free of ambient authority in M1: everything
/// they may use arrives through the [`Invocation`]. (Async execution and
/// sub-request issuing are introduced with the kernel.)
#[async_trait]
pub trait Endpoint: Send + Sync {
    /// Produce a representation for the invocation.
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation>;

    /// A short label for diagnostics.
    fn name(&self) -> &str {
        "endpoint"
    }

    /// A structured self-description, which `ikigai-vocab` can project to RDF.
    /// The default reports just the endpoint's name.
    fn describe(&self) -> Description {
        Description::new(self.name())
    }
}

/// The boxed invocation function behind a [`FnEndpoint`].
type InvokeFn = Box<dyn Fn(&Invocation<'_>) -> Result<Representation> + Send + Sync>;

/// An endpoint backed by a Rust closure — the simplest, idempotent kind.
pub struct FnEndpoint {
    name: String,
    invoke: InvokeFn,
    description: Option<Description>,
}

impl FnEndpoint {
    /// Build an endpoint from a name and an invocation function.
    pub fn new(
        name: impl Into<String>,
        invoke: impl Fn(&Invocation<'_>) -> Result<Representation> + Send + Sync + 'static,
    ) -> Self {
        FnEndpoint {
            name: name.into(),
            invoke: Box::new(invoke),
            description: None,
        }
    }

    /// Attach a self-description declaring this endpoint's parameter contract,
    /// verbs, and outputs (builder). Without one, [`Endpoint::describe`] reports
    /// just the name.
    pub fn with_description(mut self, description: Description) -> Self {
        self.description = Some(description);
        self
    }
}

#[async_trait]
impl Endpoint for FnEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        (self.invoke)(inv)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn describe(&self) -> Description {
        self.description
            .clone()
            .unwrap_or_else(|| Description::new(&self.name))
    }
}
