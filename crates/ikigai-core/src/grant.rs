//! Sub-request **delegation** for statically enumerated grants (ledger #3769,
//! `docs/design/sub-request-authority.md` Part B).
//!
//! A host names, when it builds the kernel ([`Kernel::with_grants`](crate::Kernel::with_grants)),
//! which resolved targets may act under which fixed scopes. An endpoint resolved at one of
//! those targets may then issue a sub-request under exactly that grant
//! ([`Invocation::issue_granted`](crate::Invocation::issue_granted)) instead of under its
//! caller's capability. Nothing here is request-derived: no prefix scope, no template, no
//! scope a request can name. The table is written once, by whoever constructs the kernel,
//! and every grant in it is an attenuation of a capability the host holds.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::Capability;

/// One host-declared delegation: every target whose canonical name starts with `prefix`
/// may issue sub-requests under `capability`.
#[derive(Debug)]
pub(crate) struct Grant {
    prefix: String,
    capability: Capability,
}

impl Grant {
    /// The host-declared target prefix this grant is bound to: the delegation's name on
    /// the trace and in `urn:kernel:grants`.
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The authority a delegated sub-request runs under.
    pub(crate) fn capability(&self) -> &Capability {
        &self.capability
    }
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

    /// The grant bound to the canonical `target`, if any. Prefixes are disjoint (checked at
    /// construction), so at most one matches and there is no precedence to get wrong.
    pub(crate) fn for_target(&self, target: &str) -> Option<Arc<Grant>> {
        self.entries
            .iter()
            .find(|grant| target.starts_with(grant.prefix.as_str()))
            .cloned()
    }

    /// Every grant, in the order the host declared them.
    pub(crate) fn entries(&self) -> &[Arc<Grant>] {
        &self.entries
    }

    /// This table with `declared` added, each attenuated from `host`, or the refusal.
    pub(crate) fn extend<I, P, S, T>(
        &self,
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
                .find(|g| g.prefix.starts_with(&prefix) || prefix.starts_with(&g.prefix))
            {
                return Err(GrantError::Overlaps {
                    prefix,
                    other: other.prefix.clone(),
                });
            }
            let capability = host.attenuate(scopes);
            entries.push(Arc::new(Grant { prefix, capability }));
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
    /// Two grant prefixes overlap (one is a prefix of the other, or they are equal).
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
