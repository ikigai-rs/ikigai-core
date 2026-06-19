use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// An unforgeable handle conferring authority to resolve and invoke resources.
///
/// ikigai uses no ambient authority: an endpoint receives the capabilities it
/// needs explicitly through its request context, never reaching for globals.
///
/// Authority is a set of `urn:cap:` scopes. A capability can only ever be
/// *narrowed* — [`attenuate`](Capability::attenuate) keeps a subset of the scopes
/// already held, and there is no widening operation, so non-escalation is
/// structural. The handle cannot be constructed from arbitrary data outside this
/// crate (only [`root`](Capability::root), [`scoped`](Capability::scoped), and
/// attenuation), which makes it unforgeable in-process. It also derives
/// `Serialize`/`Deserialize` so it can travel a transport — but a *deserialized*
/// capability is untrusted: the receiver must clamp it to the principal the
/// channel authenticated (e.g. the peercred-verified owner over IPC). Full
/// cryptographic unforgeability over an unauthenticated channel (QUIC) arrives
/// with capability-on-the-wire.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capability {
    kind: Kind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Kind {
    /// Full authority — grants every scope. A resource owner's root.
    Root,
    /// Exactly these `urn:cap:` scopes — a strict subset of some parent's.
    Scoped(BTreeSet<String>),
}

impl Capability {
    /// Full, unattenuated authority — grants every scope.
    pub fn root() -> Self {
        Capability { kind: Kind::Root }
    }

    /// Whether this is the full (root) authority.
    pub fn is_root(&self) -> bool {
        matches!(self.kind, Kind::Root)
    }

    /// Mint a capability bounded to exactly `scopes`.
    ///
    /// This is the trusted minting path — a host deriving a session's authority
    /// from an established identity, and (once capability-on-the-wire lands)
    /// cryptographically-verified grants arriving over a transport. It is *not*
    /// reachable by attenuation; to weaken a capability you already hold, use
    /// [`attenuate`](Capability::attenuate).
    pub fn scoped<I, S>(scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Capability {
            kind: Kind::Scoped(scopes.into_iter().map(Into::into).collect()),
        }
    }

    /// Derive a strictly-weaker capability: keep only scopes already held.
    ///
    /// `Root` attenuated to `s` yields exactly `s`; a `Scoped(t)` attenuated to
    /// `s` yields `t ∩ s`. There is no operation that widens a capability, so a
    /// holder can never produce one stronger than the one they were given.
    pub fn attenuate<I, S>(&self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let requested: BTreeSet<String> = scopes.into_iter().map(Into::into).collect();
        let kept = match &self.kind {
            Kind::Root => requested,
            Kind::Scoped(held) => requested.intersection(held).cloned().collect(),
        };
        Capability {
            kind: Kind::Scoped(kept),
        }
    }

    /// Whether this capability grants `scope`.
    ///
    /// Matching is exact today; prefix/wildcard matching over the `urn:cap:`
    /// hierarchy (so `urn:cap:personal:*:read` would cover
    /// `urn:cap:personal:calendar:read:detail`) can be added later without
    /// changing any tokens.
    pub fn allows(&self, scope: &str) -> bool {
        match &self.kind {
            Kind::Root => true,
            Kind::Scoped(held) => held.contains(scope),
        }
    }

    /// The scopes this capability grants, or `None` for root (which grants
    /// everything). For display and diagnostics.
    pub fn scopes(&self) -> Option<&BTreeSet<String>> {
        match &self.kind {
            Kind::Root => None,
            Kind::Scoped(held) => Some(held),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_root_and_allows_everything() {
        let cap = Capability::root();
        assert!(cap.is_root());
        assert!(cap.allows("urn:cap:anything:read"));
        assert!(cap.scopes().is_none());
    }

    #[test]
    fn scoped_allows_only_its_scopes() {
        let cap = Capability::scoped(["urn:cap:personal:calendar:read:freebusy"]);
        assert!(!cap.is_root());
        assert!(cap.allows("urn:cap:personal:calendar:read:freebusy"));
        assert!(!cap.allows("urn:cap:personal:calendar:read:detail"));
    }

    #[test]
    fn attenuating_root_yields_exactly_the_requested_scopes() {
        let cap = Capability::root().attenuate(["urn:cap:personal:calendar:read:freebusy"]);
        assert!(cap.allows("urn:cap:personal:calendar:read:freebusy"));
        assert!(!cap.allows("urn:cap:personal:calendar:read:detail"));
    }

    #[test]
    fn attenuation_only_narrows_never_widens() {
        let freebusy = Capability::root().attenuate(["urn:cap:personal:calendar:read:freebusy"]);
        // Asking for detail back yields nothing — you cannot widen past what you hold.
        let escalated = freebusy.attenuate([
            "urn:cap:personal:calendar:read:detail",
            "urn:cap:personal:calendar:read:freebusy",
        ]);
        assert!(!escalated.allows("urn:cap:personal:calendar:read:detail"));
        assert!(escalated.allows("urn:cap:personal:calendar:read:freebusy"));
    }

    #[test]
    fn serde_round_trips_for_the_wire() {
        // Capability travels a transport (capability-on-the-wire); it must
        // round-trip its grants intact.
        let cap = Capability::root().attenuate(["urn:cap:personal:calendar:read:freebusy"]);
        let json = serde_json::to_string(&cap).unwrap();
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert!(back.allows("urn:cap:personal:calendar:read:freebusy"));
        assert!(!back.allows("urn:cap:personal:calendar:read:detail"));
        // Root survives too.
        let root: Capability =
            serde_json::from_str(&serde_json::to_string(&Capability::root()).unwrap()).unwrap();
        assert!(root.is_root());
    }
}
