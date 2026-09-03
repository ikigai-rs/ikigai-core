use std::sync::Arc;

use serde::{Deserialize, Serialize};

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

impl Resolution {
    /// Wrap a hit's endpoint, leaving everything else the inner resolution
    /// reported intact; a miss passes through.
    ///
    /// This is the idiom for the whole interception-overlay family
    /// (`ikigai-throttle`'s `Retry`, `Timeout`, `CircuitBreaker`, …): they resolve
    /// through, then decorate the endpoint. Rebuilding a [`Resolved`] by hand
    /// instead drops [`Resolved::canonical`], and a rewrite composed underneath
    /// the overlay silently stops being one resource.
    ///
    /// ```
    /// # use ikigai_core::{Request, Resolution, Scope, Space};
    /// # fn demo(inner: &dyn Space, request: &Request, scope: &Scope) -> Resolution {
    /// inner.resolve(request, scope).map_endpoint(|endpoint| endpoint)
    /// # }
    /// ```
    pub fn map_endpoint(
        self,
        wrap: impl FnOnce(Arc<dyn Endpoint>) -> Arc<dyn Endpoint>,
    ) -> Resolution {
        match self {
            Resolution::Hit(hit) => {
                let endpoint = wrap(Arc::clone(&hit.endpoint));
                Resolution::Hit(hit.with_endpoint(endpoint))
            }
            Resolution::Miss => Resolution::Miss,
        }
    }
}

/// A successful resolution: the endpoint to invoke, its bindings, and — when the
/// resolution **rewrote** the target — the name it actually resolved under.
///
/// ★ **Build it with [`Resolved::new`], never as a struct literal.** This type
/// grows fields (`canonical` arrived in 0.1.64), and every literal in every
/// consumer is a compile break when it does. [`SpaceEntry`] taught this repo the
/// lesson at 0.1.7 — the `ikigai-web-demo` manifest still carries the epitaph.
pub struct Resolved {
    /// The resolved endpoint.
    pub endpoint: Arc<dyn Endpoint>,
    /// Variables captured by the matching grammar.
    pub bindings: Bindings,
    /// The name this resolution actually resolved under, when it differs from the
    /// request's target — `None` (the overwhelmingly common case) when nothing
    /// was rewritten.
    ///
    /// **Whoever rewrote the name reports it.** A kernel canonicalizes the
    /// request onto this name before it computes the cache key, fires the
    /// golden-thread cut, and evaluates the declared-capability floor, so a
    /// logical name and its backing name are ONE resource — one cache entry, one
    /// thread — however the rewriting overlay was composed. Before this field the
    /// only rewrite a kernel could see was one it performed itself from a table it
    /// held ([`Kernel::with_aliases`](crate::Kernel::with_aliases)); an
    /// [`Alias`](crate::Alias) composed by hand under another overlay resolved
    /// correctly and then silently split identity in two.
    ///
    /// An overlay that wraps the endpoint must carry this through — see
    /// [`Resolution::map_endpoint`] and [`Resolved::with_endpoint`], which exist
    /// so that the ergonomic thing is also the correct thing.
    pub canonical: Option<Iri>,
}

impl Resolved {
    /// A resolution that did not rewrite the target.
    pub fn new(endpoint: Arc<dyn Endpoint>, bindings: Bindings) -> Self {
        Resolved {
            endpoint,
            bindings,
            canonical: None,
        }
    }

    /// Report that this resolution rewrote the request's target to `canonical`
    /// (builder). An already-reported canonical is *kept*: the innermost rewrite
    /// is the one that names the resource actually reached.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use ikigai_core::{builtins, Bindings, Endpoint, Iri, Resolved};
    ///
    /// let endpoint: Arc<dyn Endpoint> = Arc::new(builtins::to_upper());
    /// let inner = Resolved::new(endpoint, Bindings::default())
    ///     .with_canonical(Iri::parse("urn:iki:fn:toUpper").unwrap());
    /// // An outer overlay reporting its own, shallower rewrite does not overwrite it.
    /// let outer = inner.with_canonical(Iri::parse("urn:mid:fn:toUpper").unwrap());
    /// assert_eq!(outer.canonical.unwrap().as_str(), "urn:iki:fn:toUpper");
    /// ```
    pub fn with_canonical(mut self, canonical: Iri) -> Self {
        self.canonical.get_or_insert(canonical);
        self
    }

    /// Substitute the endpoint, keeping the bindings and the reported canonical
    /// (builder). The shape a decorating overlay wants: wrapping the endpoint is
    /// not a new resolution, so it must not lose what the inner one reported.
    pub fn with_endpoint(mut self, endpoint: Arc<dyn Endpoint>) -> Self {
        self.endpoint = endpoint;
        self
    }
}

/// One binding in a space, for enumeration: the grammar's pattern and the name
/// of the endpoint it resolves to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceEntry {
    /// The grammar's pattern (an exact IRI, or a template like `…/{var}`).
    pub pattern: String,
    /// The bound endpoint's name.
    pub endpoint: String,
    /// Where this binding came from, for a **federated** catalog: `None` for this
    /// kernel's own spaces; `Some(label)` for a binding surfaced from a mounted
    /// remote (its mount alias or connection name). So an overlap reads
    /// "`urn:fn:compose` — local / via `beefybox`" instead of an anonymous
    /// concatenation, and a listing can show *where* each resource resolves.
    pub origin: Option<String>,
}

impl SpaceEntry {
    /// A binding from this kernel's own space (`origin` = `None`).
    pub fn new(pattern: impl Into<String>, endpoint: impl Into<String>) -> Self {
        SpaceEntry {
            pattern: pattern.into(),
            endpoint: endpoint.into(),
            origin: None,
        }
    }

    /// Stamp this entry's origin — a mounted remote's alias or connection name — so
    /// a federated catalog records where the binding resolves.
    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }
}

/// A space maps requests to endpoints by resolution. Spaces compose via the
/// [`Mount`], [`Fallback`], and [`Rewrite`] combinators.
pub trait Space: Send + Sync {
    /// Resolve a request to an endpoint, or report a miss.
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution;

    /// Enumerate this space's bindings, if it can. `None` means the space does
    /// not support enumeration (e.g. a rewrite or a remote space); `Some(vec![])`
    /// means it is enumerable but empty. The default is `None`.
    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        None
    }
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
                return Resolution::Hit(Resolved::new(Arc::clone(endpoint), bindings));
            }
        }
        Resolution::Miss
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        Some(
            self.bindings
                .iter()
                .map(|(grammar, endpoint)| SpaceEntry::new(grammar.pattern(), endpoint.name()))
                .collect(),
        )
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

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        // The inner space's patterns are already full identifiers.
        self.inner.entries()
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

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        // Concatenate the entries of every member that can enumerate, in order;
        // `None` only if no member supports enumeration at all.
        let mut entries = Vec::new();
        let mut enumerable = false;
        for space in &self.spaces {
            if let Some(inner) = space.entries() {
                enumerable = true;
                entries.extend(inner);
            }
        }
        enumerable.then_some(entries)
    }
}

/// The boxed rewrite rule behind a [`Rewrite`] space.
type RewriteRule = Box<dyn Fn(&Iri) -> Option<Iri> + Send + Sync>;

/// Rewrite a request's target IRI before delegating to an inner space. The rule
/// returns `Some(new_target)` to rewrite, or `None` to pass the request through
/// unchanged.
///
/// A rewrite it performs is **reported** on the [`Resolved`] as its
/// [`canonical`](Resolved::canonical) name, so a kernel keys the cache and the
/// golden thread on the backing resource rather than giving the two names an
/// entry and a thread each. See [`Alias`](crate::Alias) for the table-driven form
/// with observability, refusals, and a catalog story.
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
                rewritten.target = new_target.clone();
                match self.inner.resolve(&rewritten, scope) {
                    // Whoever rewrote the name reports it. An inner space that
                    // rewrote further has already named the resource actually
                    // reached, and `with_canonical` keeps that one.
                    Resolution::Hit(hit) => Resolution::Hit(hit.with_canonical(new_target)),
                    Resolution::Miss => Resolution::Miss,
                }
            }
            None => self.inner.resolve(request, scope),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;
    use crate::grammar::{Exact, UriTemplate};

    #[test]
    fn endpoint_space_enumerates_its_bindings() {
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:fn:toUpper"), builtins::to_upper())
            .bind(
                UriTemplate::parse("urn:demo:echo/{message}").unwrap(),
                builtins::echo(),
            );
        let entries = space.entries().expect("enumerable");
        assert_eq!(
            entries,
            vec![
                SpaceEntry::new("urn:fn:toUpper", "toUpper"),
                SpaceEntry::new("urn:demo:echo/{message}", "echo"),
            ]
        );
    }

    #[test]
    fn fallback_concatenates_enumerable_members_in_order() {
        let a = Arc::new(EndpointSpace::new().bind(Exact::new("urn:a"), builtins::to_upper()));
        let b = Arc::new(EndpointSpace::new().bind(Exact::new("urn:b"), builtins::reverse_list()));
        let entries = Fallback::new(vec![a, b]).entries().expect("enumerable");
        let patterns: Vec<&str> = entries.iter().map(|e| e.pattern.as_str()).collect();
        assert_eq!(patterns, ["urn:a", "urn:b"]);
    }

    #[test]
    fn rewrite_is_not_enumerable() {
        let inner = Arc::new(EndpointSpace::new().bind(Exact::new("urn:x"), builtins::to_upper()));
        assert!(Rewrite::new(inner, |_iri| None).entries().is_none());
    }
}
