use async_trait::async_trait;

use crate::arg::ArgRef;
use crate::capability::Capability;
use crate::describe::Description;
use crate::error::{Error, Result};
use crate::grammar::Bindings;
use crate::repr::Representation;
use crate::request::Request;

/// The context handed to an endpoint when it is invoked.
///
/// All input arrives here — the request, the bindings the resolving grammar
/// captured, and the capability authorizing the call. Endpoints take no
/// ambient authority.
pub struct Invocation<'a> {
    /// The request being served.
    pub request: &'a Request,
    /// Variables captured by the grammar that resolved this request.
    pub bindings: &'a Bindings,
    /// The capability authorizing invocation.
    pub capability: &'a Capability,
}

impl Invocation<'_> {
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
        }
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
}
