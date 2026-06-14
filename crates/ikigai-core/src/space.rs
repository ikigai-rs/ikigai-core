use std::sync::Arc;

use crate::endpoint::Endpoint;
use crate::grammar::{Bindings, Grammar};
use crate::iri::Iri;
use crate::request::Request;

/// The set of address spaces visible to a request during resolution.
///
/// In M1 it is a (possibly empty) stack of additional spaces injected into a
/// request's context; richer scope semantics (dynamic injection, pass-by-value)
/// arrive with the kernel. The resolution signature carries it from the start so
/// those mechanisms slot in without changing call sites.
#[derive(Clone, Default)]
pub struct Scope {
    injected: Vec<Arc<dyn Space>>,
}

impl Scope {
    /// An empty scope.
    pub fn empty() -> Self {
        Scope::default()
    }

    /// Inject a space into the scope (innermost last).
    pub fn with(mut self, space: Arc<dyn Space>) -> Self {
        self.injected.push(space);
        self
    }

    /// The injected spaces, innermost last.
    pub fn spaces(&self) -> &[Arc<dyn Space>] {
        &self.injected
    }
}

/// The outcome of resolving a request against a space.
pub enum Resolution {
    /// An endpoint matched, with any grammar-captured bindings.
    Hit(Resolved),
    /// Nothing in this space matched.
    Miss,
}

/// A successful resolution: the endpoint to invoke and its bindings.
pub struct Resolved {
    /// The resolved endpoint.
    pub endpoint: Arc<dyn Endpoint>,
    /// Variables captured by the matching grammar.
    pub bindings: Bindings,
}

/// A space maps requests to endpoints by resolution. Spaces compose via the
/// [`Mount`], [`Fallback`], and [`Rewrite`] combinators.
pub trait Space: Send + Sync {
    /// Resolve a request to an endpoint, or report a miss.
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution;
}

/// A leaf space: an ordered set of `(grammar, endpoint)` bindings. The first
/// grammar that matches the request's target wins.
#[derive(Default)]
pub struct EndpointSpace {
    bindings: Vec<(Box<dyn Grammar>, Arc<dyn Endpoint>)>,
}

impl EndpointSpace {
    /// An empty leaf space.
    pub fn new() -> Self {
        EndpointSpace {
            bindings: Vec::new(),
        }
    }

    /// Bind a grammar to an endpoint (builder style).
    pub fn bind(
        mut self,
        grammar: impl Grammar + 'static,
        endpoint: impl Endpoint + 'static,
    ) -> Self {
        self.bindings.push((Box::new(grammar), Arc::new(endpoint)));
        self
    }

    /// Bind a grammar to an already-shared endpoint.
    pub fn bind_arc(
        mut self,
        grammar: impl Grammar + 'static,
        endpoint: Arc<dyn Endpoint>,
    ) -> Self {
        self.bindings.push((Box::new(grammar), endpoint));
        self
    }
}

impl Space for EndpointSpace {
    fn resolve(&self, request: &Request, _scope: &Scope) -> Resolution {
        for (grammar, endpoint) in &self.bindings {
            if let Some(bindings) = grammar.match_iri(&request.target) {
                return Resolution::Hit(Resolved {
                    endpoint: Arc::clone(endpoint),
                    bindings,
                });
            }
        }
        Resolution::Miss
    }
}

/// Mount a space behind an IRI prefix; only requests whose target starts with
/// the prefix are delegated to the inner space.
pub struct Mount {
    prefix: String,
    inner: Arc<dyn Space>,
}

impl Mount {
    /// Mount `inner` at `prefix`.
    pub fn new(prefix: impl Into<String>, inner: Arc<dyn Space>) -> Self {
        Mount {
            prefix: prefix.into(),
            inner,
        }
    }
}

impl Space for Mount {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        if request.target.as_str().starts_with(&self.prefix) {
            self.inner.resolve(request, scope)
        } else {
            Resolution::Miss
        }
    }
}

/// Try each space in order; the first hit wins.
pub struct Fallback {
    spaces: Vec<Arc<dyn Space>>,
}

impl Fallback {
    /// A fallback over the given spaces, tried in order.
    pub fn new(spaces: Vec<Arc<dyn Space>>) -> Self {
        Fallback { spaces }
    }
}

impl Space for Fallback {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        for space in &self.spaces {
            if let Resolution::Hit(resolved) = space.resolve(request, scope) {
                return Resolution::Hit(resolved);
            }
        }
        Resolution::Miss
    }
}

/// The boxed rewrite rule behind a [`Rewrite`] space.
type RewriteRule = Box<dyn Fn(&Iri) -> Option<Iri> + Send + Sync>;

/// Rewrite a request's target IRI before delegating to an inner space. The rule
/// returns `Some(new_target)` to rewrite, or `None` to pass the request through
/// unchanged.
pub struct Rewrite {
    rule: RewriteRule,
    inner: Arc<dyn Space>,
}

impl Rewrite {
    /// Rewrite targets for `inner` using `rule`.
    pub fn new(
        inner: Arc<dyn Space>,
        rule: impl Fn(&Iri) -> Option<Iri> + Send + Sync + 'static,
    ) -> Self {
        Rewrite {
            rule: Box::new(rule),
            inner,
        }
    }
}

impl Space for Rewrite {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        match (self.rule)(&request.target) {
            Some(new_target) => {
                let mut rewritten = request.clone();
                rewritten.target = new_target;
                self.inner.resolve(&rewritten, scope)
            }
            None => self.inner.resolve(request, scope),
        }
    }
}
