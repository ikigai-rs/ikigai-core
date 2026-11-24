//! Sub-request **delegation** for statically enumerated grants (ledger #3769,
//! `docs/design/sub-request-authority.md` Part B).
//!
//! A host names, when it builds the kernel ([`Kernel::with_grants`](crate::Kernel::with_grants)),
//! which of its endpoints may act under which fixed scopes. An endpoint the grant was given to
//! may then issue a sub-request under exactly that grant
//! ([`Invocation::issue_granted`](crate::Invocation::issue_granted)) instead of under its
//! caller's capability. Nothing here is request-derived: no prefix scope, no template, no
//! scope a request can name. The table is written once, by whoever constructs the kernel,
//! and every grant in it is an attenuation of a capability the host holds.
//!
//! ★ **A grant belongs to endpoints, not to names** (hub review of PR 333, round 3). The
//! host's prefix only SELECTS, at construction, the exact bindings of the root that the grant
//! is given to: each is resolved then, and the grant records the name and the identity of the
//! endpoint that answered it ([`Door`]). At dispatch a grant applies only when the answering
//! endpoint IS one of those, at the name it was enumerated under. Whatever else answers under
//! the prefix (a template or fallback binding, a mounted peer, a door a dynamic space swaps in
//! later) gets no grant, and its delegated hop is `Denied`.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{Capability, Endpoint, Iri, Request, Resolution, Scope, Space, Verb};

/// Whether the host-declared `prefix` covers `name` **on a segment boundary**: `name` equals
/// it, or continues it after a `:` or `/` (the prefix's own last character, or the next one
/// of `name`). So `urn:dash` and `urn:dash:` both cover `urn:dash:prices`, and neither covers
/// `urn:dashboard-other:x`.
pub(crate) fn covers(prefix: &str, name: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    rest.is_empty() || prefix.ends_with([':', '/']) || rest.starts_with([':', '/'])
}

/// One endpoint a grant was given to: the exact name the host's root bound it at and the
/// endpoint that name resolved to when the table was built. Holding the `Arc` reserves its
/// address for the kernel's lifetime, so the pointer identity cannot be reused by another
/// endpoint.
#[derive(Clone)]
pub(crate) struct Door {
    name: String,
    endpoint: Arc<dyn Endpoint>,
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Door")
            .field("name", &self.name)
            .field("endpoint", &self.endpoint.name())
            .finish()
    }
}

/// One host-declared delegation: the exact doors under `prefix` that the host's root bound
/// when the table was built may issue sub-requests under `capability`.
#[derive(Debug)]
pub(crate) struct Grant {
    prefix: String,
    capability: Capability,
    doors: Vec<Door>,
}

impl Grant {
    /// The host-declared target prefix this grant was declared for: the delegation's name on
    /// the trace and in `urn:kernel:grants`.
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The authority a delegated sub-request runs under.
    pub(crate) fn capability(&self) -> &Capability {
        &self.capability
    }

    /// The exact names this grant was given to, in the root's enumeration order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.doors.iter().map(|door| door.name.as_str())
    }

    /// Whether `endpoint`, answering at the canonical `target`, is one of this grant's doors.
    fn given_to(&self, target: &str, endpoint: &Arc<dyn Endpoint>) -> bool {
        self.doors
            .iter()
            .any(|door| door.name == target && Arc::ptr_eq(&door.endpoint, endpoint))
    }
}

/// The endpoint the root answers the exact `name` with, when its identity can be established:
/// a hit that captured no template variables, reported no rewrite, and answers with the SAME
/// endpoint when asked twice. A space that builds a fresh endpoint per resolution (a mount's
/// forwarder, a dynamic space's threaded wrapper) or answers through a pattern has no identity
/// to grant to, so the binding is refused here, never granted at dispatch.
fn identify(root: &Arc<dyn Space>, name: &str) -> Option<Arc<dyn Endpoint>> {
    let iri = Iri::parse(name).ok()?;
    let request = Request::new(Verb::Source, iri);
    let answer = || match root.resolve(&request, &Scope::empty()) {
        Resolution::Hit(hit) if hit.bindings.is_empty() && hit.canonical.is_none() => {
            Some(hit.endpoint)
        }
        _ => None,
    };
    let first = answer()?;
    let second = answer()?;
    Arc::ptr_eq(&first, &second).then_some(first)
}

/// The kernel's grant table: disjoint prefixes, each with the scopes the host delegated.
#[derive(Debug, Default, Clone)]
pub(crate) struct Grants {
    entries: Vec<Arc<Grant>>,
}

impl Grants {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The grant whose declared prefix covers the canonical `target` on a segment boundary,
    /// if any. Prefixes are disjoint (checked at construction), so at most one matches and
    /// there is no precedence to get wrong. This is the NAME half only: whether the endpoint
    /// answering there was given the grant is [`given`](Self::given).
    pub(crate) fn for_target(&self, target: &str) -> Option<Arc<Grant>> {
        self.entries
            .iter()
            .find(|grant| covers(&grant.prefix, target))
            .cloned()
    }

    /// The grant `endpoint`, answering at the canonical `target`, was given at construction,
    /// if any: the one enumerated door of the table that is this name AND this endpoint.
    pub(crate) fn given(&self, target: &str, endpoint: &Arc<dyn Endpoint>) -> Option<Arc<Grant>> {
        self.for_target(target)
            .filter(|grant| grant.given_to(target, endpoint))
    }

    /// The grant given to the door enumerated at the exact `target`, if any.
    pub(crate) fn at_door(&self, target: &str) -> Option<Arc<Grant>> {
        self.for_target(target)
            .filter(|grant| grant.names().any(|name| name == target))
    }

    /// Every grant, in the order the host declared them.
    pub(crate) fn entries(&self) -> &[Arc<Grant>] {
        &self.entries
    }

    /// This table with `declared` added, each attenuated from `host` and given to the exact
    /// bindings of `root` under its prefix whose endpoint identity can be established
    /// ([`identify`]), or the refusal.
    pub(crate) fn extend<I, P, S, T>(
        &self,
        root: &Arc<dyn Space>,
        host: &Capability,
        declared: I,
    ) -> std::result::Result<Grants, GrantError>
    where
        I: IntoIterator<Item = (P, S)>,
        P: Into<String>,
        S: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let mut entries = self.entries.clone();
        for (prefix, scopes) in declared {
            let prefix: String = prefix.into();
            let scopes: BTreeSet<String> = scopes.into_iter().map(Into::into).collect();
            if prefix.is_empty() {
                return Err(GrantError::EmptyPrefix);
            }
            if scopes.is_empty() {
                return Err(GrantError::NoScopes { prefix });
            }
            // ★ A delegation cannot exceed what the host holds: the grant is the host's
            // own capability attenuated to the scopes, and a scope the host does not hold
            // is refused here rather than silently dropped (a dropped scope would surface
            // as a denial far from the wiring that caused it).
            if let Some(missing) = scopes.iter().find(|scope| !host.allows(scope)) {
                return Err(GrantError::NotHeld {
                    prefix,
                    scope: missing.clone(),
                });
            }
            // ★ No prefix may be a prefix of another. Longest-match is a resolution rule;
            // for authority it is silent precedence, and silent precedence over authority
            // is how a grant ends up broader than anyone read it as.
            if let Some(other) = entries
                .iter()
                .find(|g| covers(&g.prefix, &prefix) || covers(&prefix, &g.prefix))
            {
                return Err(GrantError::Overlaps {
                    prefix,
                    other: other.prefix.clone(),
                });
            }
            let capability = host.attenuate(scopes);
            // ★ The endpoints the grant is GIVEN to, fixed now: the root's own exact
            // bindings under the prefix (a binding surfaced from a mounted remote is not the
            // host's endpoint, and a pattern names no single endpoint). A request, a later
            // swap or another binding under the prefix cannot add to this list.
            let doors = root
                .entries()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry.origin.is_none() && !entry.pattern.contains('{'))
                .filter(|entry| covers(&prefix, &entry.pattern))
                .filter_map(|entry| {
                    identify(root, &entry.pattern).map(|endpoint| Door {
                        name: entry.pattern,
                        endpoint,
                    })
                })
                .fold(Vec::<Door>::new(), |mut doors, door| {
                    if !doors.iter().any(|d| d.name == door.name) {
                        doors.push(door);
                    }
                    doors
                });
            entries.push(Arc::new(Grant {
                prefix,
                capability,
                doors,
            }));
        }
        Ok(Grants { entries })
    }
}

/// Why [`Kernel::try_with_grants`](crate::Kernel::try_with_grants) refused a grant table.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GrantError {
    /// A grant bound to the empty prefix would delegate to every target.
    EmptyPrefix,
    /// A grant with no scopes delegates nothing; it is almost certainly a wiring mistake.
    NoScopes {
        /// The target prefix the empty grant was declared for.
        prefix: String,
    },
    /// The host's own capability does not hold a scope it tried to delegate.
    NotHeld {
        /// The target prefix the grant was declared for.
        prefix: String,
        /// The scope the host does not hold.
        scope: String,
    },
    /// Two grant prefixes overlap (one covers the other on a segment boundary, or they are
    /// equal).
    Overlaps {
        /// The prefix being added.
        prefix: String,
        /// The prefix already in the table that it overlaps.
        other: String,
    },
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantError::EmptyPrefix => {
                write!(f, "a grant must name a target prefix; the empty prefix names every target")
            }
            GrantError::NoScopes { prefix } => {
                write!(f, "the grant for `{prefix}` names no scopes")
            }
            GrantError::NotHeld { prefix, scope } => write!(
                f,
                "the grant for `{prefix}` names `{scope}`, which the host's capability does not hold"
            ),
            GrantError::Overlaps { prefix, other } => write!(
                f,
                "the grant for `{prefix}` overlaps the grant for `{other}`; grant prefixes must be disjoint"
            ),
        }
    }
}

impl std::error::Error for GrantError {}
