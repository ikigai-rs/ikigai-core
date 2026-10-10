use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// An unforgeable handle conferring authority to resolve and invoke resources.
///
/// ikigai uses no ambient authority: an endpoint receives the capabilities it
/// needs explicitly through its request context, never reaching for globals.
///
/// Authority is a set of `urn:cap:` scopes. A capability can only ever be
/// *narrowed* — [`attenuate`](Capability::attenuate) keeps a subset of the grants
/// already held, and there is no widening operation, so non-escalation is
/// structural. The handle cannot be constructed from arbitrary data outside this
/// crate (only [`root`](Capability::root), [`scoped`](Capability::scoped), and
/// attenuation), which makes it unforgeable in-process. It also derives
/// `Serialize`/`Deserialize` so it can travel a transport — but a *deserialized*
/// capability is untrusted: the receiver must clamp it to the principal the
/// channel authenticated (e.g. the peercred-verified owner over IPC). Full
/// cryptographic unforgeability over an unauthenticated channel (QUIC) arrives
/// with capability-on-the-wire.
///
/// # Exclusions are sticky
///
/// A scope set holds two kinds of token. Most are GRANTS. Some are EXCLUSIONS: a
/// parameterized grammar spells "everything under this allow except that" as a
/// separate scope with a leading `-` on its rule, and the module resolves the two by
/// specificity (ikigai-fs: `urn:cap:fs:read:/root` + `urn:cap:fs:read:-/root/secret`;
/// ikigai-http: `urn:cap:net:example.com` + `urn:cap:net:-example.com/admin`; the
/// per-tenant fs rules ikigai-cli localizes). [`is_deny_scope`] is the one definition
/// of that shape. Dropping an exclusion WIDENS what the holder can reach, so the two
/// narrowing operations treat the kinds oppositely: **grants only shrink, exclusions
/// only accumulate**. [`attenuate`](Capability::attenuate) and
/// [`clamp`](Capability::clamp) keep every exclusion either side holds, and keep a
/// grant only when both sides name it (ledger #858). Exclusions are not grants:
/// [`allows`](Capability::allows) never answers yes to one, so a `requires` must never
/// name a deny-shaped token (only root could satisfy it).
///
/// ```
/// use ikigai_core::Capability;
///
/// let parent = Capability::scoped(["urn:cap:fs:read:/root", "urn:cap:fs:read:-/root/secret"]);
/// // A delegate asks for the allow alone; the exclusion comes along regardless.
/// let child = parent.attenuate(["urn:cap:fs:read:/root"]);
/// assert_eq!(child, parent);
/// // A peer that carries only the allow is clamped to the same thing.
/// assert_eq!(parent.clamp(&Capability::scoped(["urn:cap:fs:read:/root"])), parent);
/// ```
///
/// # Principals: who a request is for
///
/// Identity travels as a scope, minted by the door that authenticated it (ledger #1077):
/// `urn:cap:principal:<iri>` ([`PRINCIPAL_SCOPE_PREFIX`], [`principal_scope`]). Not a
/// field on the request, so everything a capability already does applies to it: the
/// cache partitions by it (two principals hold two capabilities), a wire carries it
/// (the serialized form is the same flat scope set), and an endpoint reads it from the
/// capability it was handed ([`principal`](Capability::principal),
/// [`acts_as`](Capability::acts_as)).
///
/// - **`<iri>` is an absolute IRI, carried verbatim**: never escaped, never normalized,
///   compared as a string. An escape would give one identity two spellings, and two
///   spellings are two principals that each fail the other's check. What cannot be
///   carried verbatim is refused at minting ([`PrincipalError`]): not an absolute IRI,
///   longer than [`MAX_PRINCIPAL_LEN`], containing `*` (the family-wildcard character),
///   or spelling a scope [`is_deny_scope`] reads as an exclusion (`x:-y`).
/// - **One principal per capability.** A request is made for one party; attribution
///   (ikigai-log's `principal=` column) holds one answer. A capability holding SEVERAL
///   well-formed principal scopes holds NONE: [`principal`](Capability::principal) is
///   `None`, [`allows`](Capability::allows) grants none of them, and narrowing drops
///   them all, so an ambiguous capability can never be narrowed into one of its names.
/// - **Minting is a door's act** ([`with_principal`](Capability::with_principal)), in the
///   same trust class as [`scoped`](Capability::scoped). It replaces whatever the
///   capability held under the prefix.
/// - **Narrowing never adds or changes a principal.** [`attenuate`](Capability::attenuate)
///   keeps the held principal only when the request names it (shedding it is narrowing,
///   like dropping any grant: the sub-request goes out anonymous) and never takes one
///   from the request. [`clamp`](Capability::clamp) is a door's operation and keeps the
///   CEILING's principal whatever the carried capability says: a client that carries a
///   narrower capability of its own cannot shed the identity its channel authenticated
///   (the failure ledger #879 hit when gonk tagged the capability and a client's own
///   capability intersected the tag away), and cannot claim another one.
/// - **Root holds every principal and names none.** `principal()` of root is `None`
///   (root is the host's own authority, not a party's), `acts_as(x)` of root is `true`
///   for every `x` (root sees every author's draft), and root narrows to any one
///   principal by attenuation, which is how a root-holding host mints. Minting onto root
///   itself is refused: a root capability cannot carry a name.
/// - **A wildcard is never an identity.** A held `urn:cap:principal:*` or `urn:cap:*`
///   names nobody: `principal()` ignores it and `acts_as` is false for every name. A
///   declared requirement `urn:cap:principal:*` is the usual PRESENCE test (see
///   [`allows`](Capability::allows)), so an endpoint that asks "who" reads
///   `principal()`, never the presence test.
///
/// ```
/// use ikigai_core::{principal_scope, Capability};
///
/// assert_eq!(
///     principal_scope("urn:example:person:alice").unwrap(),
///     "urn:cap:principal:urn:example:person:alice"
/// );
/// // A door mints the identity it authenticated onto the session's authority.
/// let session = Capability::scoped(["urn:cap:script:read"])
///     .with_principal("urn:example:person:alice")
///     .unwrap();
/// assert_eq!(session.principal(), Some("urn:example:person:alice"));
/// // A client carrying a narrower capability of its own keeps the door's name...
/// let carried = Capability::scoped(["urn:cap:script:read", "urn:cap:principal:urn:example:person:bob"]);
/// assert_eq!(session.clamp(&carried).principal(), Some("urn:example:person:alice"));
/// // ...and narrowing can drop the name, but never change it.
/// assert_eq!(session.attenuate(["urn:cap:script:read"]).principal(), None);
/// let renamed = session.attenuate(["urn:cap:principal:urn:example:person:bob"]);
/// assert_eq!(renamed.principal(), None);
/// // Root names nobody and acts as everyone.
/// assert_eq!(Capability::root().principal(), None);
/// assert!(Capability::root().acts_as("urn:example:person:alice"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Kind {
    /// Full authority — grants every scope. A resource owner's root.
    Root,
    /// Exactly these `urn:cap:` scopes — grants and deny-shaped exclusions together
    /// (see [`is_deny_scope`]). Serialized as one flat set, unchanged since before
    /// exclusions were told apart, so the wire carries no new shape.
    Scoped(BTreeSet<String>),
}

/// Whether `scope` is DENY-SHAPED: an exclusion, which narrowing never drops.
///
/// The shape is the convention every module that spells exclusions already shares —
/// a leading `-` on the rule, in one of exactly two positions:
///
/// - `urn:cap:<family>:-<rule>` (ikigai-http: `urn:cap:net:-internal.example`), or
/// - `urn:cap:<family>:<qualifier>:-<rule>` (ikigai-fs and the ikigai-cli tenant
///   rules: `urn:cap:fs:read:-/root/secret`, `urn:cap:fs:write:-notes/private`),
///
/// where `<family>` and `<qualifier>` are WORDS: ASCII lowercase letters, digits, `_`
/// and `-`, starting with a letter or digit. `<rule>` is anything, empty included.
/// Nothing later in a scope counts: a `-` deeper in (`urn:cap:fs:read:notes:-x`, a
/// graph IRI under `urn:cap:store:read:graph:`) is part of a parameter, and a `-`
/// inside a segment (`urn:cap:fs:read:/a-b`) is just a character.
///
/// ⚠ The two positions are therefore RESERVED across `urn:cap:`. A grammar whose
/// parameter sits there (`urn:cap:secret:read:<name>`) must not let that parameter
/// begin with `-`, because core will treat such a token as an exclusion: kept through
/// every narrowing, never granted by [`Capability::allows`]. Failing closed is the
/// point — misreading a grant as an exclusion denies; misreading an exclusion as a
/// grant would widen.
///
/// ```
/// use ikigai_core::is_deny_scope;
///
/// // ikigai-fs and the tenant rules, every action, absolute and relative.
/// assert!(is_deny_scope("urn:cap:fs:read:-/root/secret"));
/// assert!(is_deny_scope("urn:cap:fs:write:-/root/secret"));
/// assert!(is_deny_scope("urn:cap:fs:read:-secret"));
/// // ikigai-http host rules, with a port, a path, or an IPv6 literal.
/// assert!(is_deny_scope("urn:cap:net:-example.com"));
/// assert!(is_deny_scope("urn:cap:net:-example.com:443/admin"));
/// assert!(is_deny_scope("urn:cap:net:-[::1]:8080"));
/// // An empty rule is still an exclusion (it excludes nothing; ikigai-http ignores it).
/// assert!(is_deny_scope("urn:cap:net:-"));
///
/// // Grants, whatever `-` they contain elsewhere.
/// assert!(!is_deny_scope("urn:cap:fs:read:/root"));
/// assert!(!is_deny_scope("urn:cap:fs:read:/a-b"));
/// assert!(!is_deny_scope("urn:cap:fs:read:notes:-x"));
/// assert!(!is_deny_scope("urn:cap:net:example.com:-1"));
/// assert!(!is_deny_scope("urn:cap:store:read:graph:urn:x:-y"));
/// assert!(!is_deny_scope("urn:cap:dev-server:read"));
/// // Wildcard requirements and things that are not capability scopes at all.
/// assert!(!is_deny_scope("urn:cap:fs:read:*"));
/// assert!(!is_deny_scope("urn:cap:net:*"));
/// assert!(!is_deny_scope("urn:cap:-fs:read"));
/// assert!(!is_deny_scope("urn:other:fs:-x"));
/// ```
pub fn is_deny_scope(scope: &str) -> bool {
    let Some(rest) = scope.strip_prefix("urn:cap:") else {
        return false;
    };
    let mut segments = rest.splitn(3, ':');
    let family = segments.next().unwrap_or_default();
    if !is_word(family) {
        return false;
    }
    match segments.next() {
        Some(second) if second.starts_with('-') => true,
        Some(qualifier) if is_word(qualifier) => {
            segments.next().is_some_and(|rule| rule.starts_with('-'))
        }
        _ => false,
    }
}

/// A family or qualifier segment: `[a-z0-9][a-z0-9_-]*`.
fn is_word(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The prefix of a principal scope: `urn:cap:principal:<iri>`. See [`Capability`]'s
/// "Principals" section for the rules.
pub const PRINCIPAL_SCOPE_PREFIX: &str = "urn:cap:principal:";

/// The longest principal IRI a capability will carry, in bytes: the bound ikigai-log
/// writes in its `principal=` column, so every principal a door mints is one the log
/// can record. A bound that refuses rather than truncates.
pub const MAX_PRINCIPAL_LEN: usize = 512;

/// Why an IRI cannot be carried as a principal ([`principal_scope`],
/// [`Capability::with_principal`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrincipalError {
    /// Not an absolute IRI (RFC 3987), so not a name for anyone.
    NotAnIri {
        /// What was offered.
        offered: String,
        /// Why the parser refused it.
        detail: String,
    },
    /// Longer than [`MAX_PRINCIPAL_LEN`] bytes.
    TooLong {
        /// Its length in bytes.
        length: usize,
    },
    /// Contains `*`, the character a declared requirement uses as a family wildcard.
    Wildcard {
        /// What was offered.
        offered: String,
    },
    /// Its scope would be deny-shaped ([`is_deny_scope`]), so core would keep it as an
    /// exclusion and never grant it.
    DenyShaped {
        /// What was offered.
        offered: String,
    },
    /// Minting onto root: root holds every principal and cannot carry one name.
    /// Attenuate to the session's scopes first, then mint.
    Root,
}

impl std::fmt::Display for PrincipalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrincipalError::NotAnIri { offered, detail } => {
                write!(
                    f,
                    "{offered:?} is not an absolute IRI, so it names no principal: {detail}"
                )
            }
            PrincipalError::TooLong { length } => write!(
                f,
                "a principal may be at most {MAX_PRINCIPAL_LEN} bytes and this one is {length}"
            ),
            PrincipalError::Wildcard { offered } => write!(
                f,
                "{offered:?} contains `*`, which a capability reads as a wildcard, never a name"
            ),
            PrincipalError::DenyShaped { offered } => write!(
                f,
                "{offered:?} would spell a deny-shaped scope, which is never granted"
            ),
            PrincipalError::Root => f.write_str(
                "root holds every principal and cannot carry one; attenuate it to the \
                 session's scopes, then mint the principal onto that",
            ),
        }
    }
}

impl std::error::Error for PrincipalError {}

/// The scope that carries `iri` as a principal: `urn:cap:principal:<iri>`, verbatim, or
/// why `iri` cannot be one. See [`Capability`]'s "Principals" section.
///
/// ```
/// use ikigai_core::{principal_scope, PrincipalError};
///
/// assert_eq!(
///     principal_scope("https://example.org/people/brian#me").unwrap(),
///     "urn:cap:principal:https://example.org/people/brian#me"
/// );
/// assert_eq!(
///     principal_scope("urn:example:passkey:AbC").unwrap(),
///     "urn:cap:principal:urn:example:passkey:AbC"
/// );
/// // Refused, never escaped: one identity has one spelling.
/// assert!(matches!(principal_scope("alice"), Err(PrincipalError::NotAnIri { .. })));
/// assert!(matches!(principal_scope("urn:x:two words"), Err(PrincipalError::NotAnIri { .. })));
/// assert!(matches!(principal_scope("*"), Err(PrincipalError::NotAnIri { .. })));
/// assert!(matches!(principal_scope("urn:x:*"), Err(PrincipalError::Wildcard { .. })));
/// assert!(matches!(principal_scope("x:-y"), Err(PrincipalError::DenyShaped { .. })));
/// ```
pub fn principal_scope(iri: &str) -> Result<String, PrincipalError> {
    check_principal(iri)?;
    Ok(format!("{PRINCIPAL_SCOPE_PREFIX}{iri}"))
}

fn check_principal(iri: &str) -> Result<(), PrincipalError> {
    if iri.len() > MAX_PRINCIPAL_LEN {
        return Err(PrincipalError::TooLong { length: iri.len() });
    }
    if let Err(e) = crate::Iri::parse(iri) {
        return Err(PrincipalError::NotAnIri {
            offered: iri.to_string(),
            detail: e.to_string(),
        });
    }
    if iri.contains('*') {
        return Err(PrincipalError::Wildcard {
            offered: iri.to_string(),
        });
    }
    if is_deny_scope(&format!("{PRINCIPAL_SCOPE_PREFIX}{iri}")) {
        return Err(PrincipalError::DenyShaped {
            offered: iri.to_string(),
        });
    }
    Ok(())
}

/// The principal a scope carries, if it is a well-formed principal scope.
fn principal_in(scope: &str) -> Option<&str> {
    let iri = scope.strip_prefix(PRINCIPAL_SCOPE_PREFIX)?;
    check_principal(iri).ok().map(|()| iri)
}

/// The one principal `held` carries: `None` for none and for several.
fn single_principal(held: &BTreeSet<String>) -> Option<&str> {
    let mut principals = held.iter().filter_map(|scope| principal_in(scope));
    let first = principals.next()?;
    principals.next().is_none().then_some(first)
}

/// Drop every principal scope from `scopes` if it carries more than one: a narrowing
/// never yields an ambiguous capability.
fn at_most_one_principal(mut scopes: BTreeSet<String>) -> BTreeSet<String> {
    if scopes.iter().filter(|s| principal_in(s).is_some()).count() > 1 {
        scopes.retain(|s| principal_in(s).is_none());
    }
    scopes
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

    /// Derive a strictly-weaker capability: keep only grants already held, and every
    /// exclusion.
    ///
    /// `Root` attenuated to `s` yields exactly `s`, exclusions included. A `Scoped(t)`
    /// attenuated to `s` yields the grants in `t ∩ s`, plus every exclusion `t` holds
    /// (a holder cannot shed one), plus every exclusion `s` adds (that only narrows).
    /// An exclusion is a scope [`is_deny_scope`] recognizes; why they are sticky is on
    /// [`Capability`]. There is no operation that widens a capability, so a holder can
    /// never produce one stronger than the one they were given.
    pub fn attenuate<I, S>(&self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let requested: BTreeSet<String> = scopes.into_iter().map(Into::into).collect();
        let kept = match &self.kind {
            // Root holds every principal, so it narrows to any one; to several, none.
            Kind::Root => at_most_one_principal(requested),
            Kind::Scoped(held) => {
                // A principal survives only as the holder's ONE principal, and only when
                // asked for: never added, never changed, never picked out of several.
                let mine = single_principal(held);
                let mut kept: BTreeSet<String> = held
                    .iter()
                    .filter(|scope| match principal_in(scope) {
                        Some(principal) => Some(principal) == mine && requested.contains(*scope),
                        None => is_deny_scope(scope) || requested.contains(*scope),
                    })
                    .cloned()
                    .collect();
                kept.extend(requested.into_iter().filter(|scope| is_deny_scope(scope)));
                kept
            }
        };
        Capability {
            kind: Kind::Scoped(kept),
        }
    }

    /// Clamp to a ceiling: the strongest capability **both** `self` and `ceiling`
    /// grant. Used to bound an untrusted, transport-carried capability to the
    /// authority the channel authenticated — a peer can present any capability, but
    /// the server resolves under `ceiling.clamp(&carried)`, so it can only ever
    /// *narrow* its own authority, never exceed the principal it authenticated as.
    /// `ceiling.clamp(root) = ceiling`; `root.clamp(c) = c`; otherwise the grants both
    /// name plus the exclusions of EITHER side (via [`attenuate`](Self::attenuate), so
    /// it never widens): a peer cannot clamp its way out of an exclusion the ceiling
    /// holds, and an exclusion the peer carries still narrows it.
    ///
    /// The principal is the CEILING's (see "Principals" on [`Capability`]): a scoped
    /// ceiling's one principal survives whatever the peer carried, and a principal the
    /// peer carried never enters. Under a root ceiling the result is the carried
    /// capability, principal included, as it always was: root may act as anyone. A host
    /// clamping a capability it recorded itself (a scheduled job's), rather than one a
    /// peer presented, re-mints the recorded principal after the clamp.
    pub fn clamp(&self, carried: &Capability) -> Capability {
        match carried.scopes() {
            // The peer carried root — clamp to our own ceiling.
            None => self.clone(),
            // Keep only grants the ceiling already holds, and every exclusion.
            Some(scopes) => {
                let mut clamped = self.attenuate(scopes.iter().cloned());
                if let (Kind::Scoped(held), Kind::Scoped(kept)) = (&self.kind, &mut clamped.kind) {
                    if let Some(principal) = single_principal(held) {
                        kept.insert(format!("{PRINCIPAL_SCOPE_PREFIX}{principal}"));
                    }
                }
                clamped
            }
        }
    }

    /// The one principal this capability carries (see "Principals" on [`Capability`]):
    /// `None` for root, for none, and for several.
    pub fn principal(&self) -> Option<&str> {
        match &self.kind {
            Kind::Root => None,
            Kind::Scoped(held) => single_principal(held),
        }
    }

    /// Whether this capability acts as `principal`: root for everyone, any other
    /// capability exactly when [`principal`](Self::principal) names it. The check an
    /// author-scoped resource makes ("may this caller see alice's draft?").
    pub fn acts_as(&self, principal: &str) -> bool {
        match &self.kind {
            Kind::Root => true,
            Kind::Scoped(held) => single_principal(held) == Some(principal),
        }
    }

    /// The same authority carrying `principal` as its identity, replacing every scope it
    /// held under [`PRINCIPAL_SCOPE_PREFIX`]: the door's mint, in the same trust class as
    /// [`scoped`](Self::scoped), and unreachable by narrowing. Refused for an IRI
    /// [`principal_scope`] refuses, and for root, which holds every principal and cannot
    /// carry one name.
    ///
    /// ```
    /// use ikigai_core::{Capability, PrincipalError};
    ///
    /// let held = Capability::scoped([
    ///     "urn:cap:ledger:read:default",
    ///     "urn:cap:principal:urn:example:old",
    ///     "urn:cap:principal:*",
    /// ]);
    /// let minted = held.with_principal("urn:example:person:alice").unwrap();
    /// assert_eq!(
    ///     minted,
    ///     Capability::scoped([
    ///         "urn:cap:ledger:read:default",
    ///         "urn:cap:principal:urn:example:person:alice",
    ///     ])
    /// );
    /// assert_eq!(
    ///     Capability::root().with_principal("urn:example:person:alice"),
    ///     Err(PrincipalError::Root)
    /// );
    /// ```
    pub fn with_principal(&self, principal: &str) -> Result<Capability, PrincipalError> {
        let scope = principal_scope(principal)?;
        match &self.kind {
            Kind::Root => Err(PrincipalError::Root),
            Kind::Scoped(held) => {
                let mut minted: BTreeSet<String> = held
                    .iter()
                    .filter(|s| !s.starts_with(PRINCIPAL_SCOPE_PREFIX))
                    .cloned()
                    .collect();
                minted.insert(scope);
                Ok(Capability {
                    kind: Kind::Scoped(minted),
                })
            }
        }
    }

    /// Whether this capability grants `scope`, matched **exactly**.
    ///
    /// This is the primitive, not the whole predicate. Enforcement, selection, and
    /// `urn:kernel:validate` all go through `select::cap_satisfies`, which layers the
    /// trailing-`*` family form on top of this: `urn:cap:net:*` means "holds ANY grant
    /// under this prefix." That form is how a parameterized capability grammar
    /// (`urn:cap:net:<host-rule>`, `urn:cap:fs:<action>:<path>`) annotates an action
    /// whose authorization depends on an argument, so a declared `urn:cap:net:*` IS
    /// satisfied by a held `urn:cap:net:example.com` — through `cap_satisfies`, never
    /// through this method. Callers wanting the enforced semantics want that one.
    ///
    /// ⚠ The family form is a PRESENCE test, and an exclusion is present: a holder of
    /// nothing but `urn:cap:fs:read:-/secret` satisfies a declared `urn:cap:fs:read:*`,
    /// so the action is offered and dispatched, and the module's own rule (no allow
    /// covers the path) refuses it. That is deliberate and unchanged; the floor says
    /// "may hold a grant here", the module says which.
    ///
    /// An exclusion ([`is_deny_scope`]) is never granted by this method, held or not:
    /// it is a narrowing, so a `requires` that names one is satisfiable only by root.
    /// Exclusions are read through [`scopes`](Self::scopes) by the module that spells
    /// them.
    ///
    /// A principal scope ([`principal_scope`]) is granted only as the one principal the
    /// capability carries: held among several, it is not granted (see "Principals" on
    /// [`Capability`]).
    ///
    /// What is still absent is INFIX matching over the hierarchy — `urn:cap:personal:*:read`
    /// covering `urn:cap:personal:calendar:read:detail`. That can be added later without
    /// changing any tokens.
    pub fn allows(&self, scope: &str) -> bool {
        match &self.kind {
            Kind::Root => true,
            Kind::Scoped(held) => {
                held.contains(scope)
                    && !is_deny_scope(scope)
                    // A principal is granted only as the ONE the capability carries.
                    && principal_in(scope).is_none_or(|p| single_principal(held) == Some(p))
            }
        }
    }

    /// The scopes this capability holds — grants and exclusions alike — or `None` for
    /// root (which grants everything). For modules that interpret their own grammar
    /// (path and host rules), for display, and for diagnostics.
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

    /// Ledger #858, reproduced in core: an allow plus a deny-shaped scope (ikigai-fs's
    /// form). Attenuating to the allow alone, or clamping against a carried capability
    /// that names only the allow, must keep the deny.
    #[test]
    fn attenuate_and_clamp_keep_a_held_exclusion() {
        let parent = Capability::scoped(["urn:cap:fs:read:/root", "urn:cap:fs:read:-/root/secret"]);
        let child = parent.attenuate(["urn:cap:fs:read:/root"]);
        assert!(
            child
                .scopes()
                .unwrap()
                .contains("urn:cap:fs:read:-/root/secret"),
            "attenuate dropped the deny: {child:?}"
        );
        let clamped = parent.clamp(&Capability::scoped(["urn:cap:fs:read:/root"]));
        assert!(
            clamped
                .scopes()
                .unwrap()
                .contains("urn:cap:fs:read:-/root/secret"),
            "clamp dropped the deny: {clamped:?}"
        );
    }

    #[test]
    fn clamp_bounds_a_carried_capability_to_the_ceiling() {
        let ceiling = Capability::root().attenuate(["a".to_string(), "b".to_string()]);

        // A peer carrying root is clamped down to the ceiling — it cannot exceed it.
        let c = ceiling.clamp(&Capability::root());
        assert!(c.allows("a") && c.allows("b") && !c.allows("c"));

        // A peer carrying a broader set keeps only what the ceiling also grants.
        let c = ceiling.clamp(&Capability::root().attenuate(["a".to_string(), "c".to_string()]));
        assert!(c.allows("a") && !c.allows("b") && !c.allows("c"));

        // A root ceiling clamps to exactly what the peer carried.
        let c = Capability::root().clamp(&Capability::root().attenuate(["a".to_string()]));
        assert!(c.allows("a") && !c.allows("b"));
    }

    /// Every deny form the three users spell, and the grants that look like them.
    /// The doctest on `is_deny_scope` shows the rule; this pins each real form.
    #[test]
    fn deny_shape_pins_every_real_form() {
        let denies = [
            // ikigai-fs, every action it names, absolute and relative.
            "urn:cap:fs:read:-/root/secret",
            "urn:cap:fs:write:-/root/secret",
            "urn:cap:fs:delete:-/root/secret",
            "urn:cap:fs:list:-/root/secret",
            // ikigai-cli tenant rules: relative before localizing, rooted after.
            "urn:cap:fs:read:-secret",
            "urn:cap:fs:read:-/Users/brian/secrets",
            // ikigai-http host rules.
            "urn:cap:net:-example.com",
            "urn:cap:net:-example.com:443",
            "urn:cap:net:-example.com/admin",
            "urn:cap:net:-example.com:443/admin",
            "urn:cap:net:-[::1]:8080",
            "urn:cap:net:-",
        ];
        for scope in denies {
            assert!(is_deny_scope(scope), "{scope} is an exclusion");
        }
        let grants = [
            "urn:cap:fs:read:/root",
            "urn:cap:fs:read:/a-b",
            "urn:cap:fs:read:a-b",
            "urn:cap:fs:read:notes:-x",
            "urn:cap:fs:read:/a:-b",
            "urn:cap:fs:read:*",
            "urn:cap:fs:read",
            "urn:cap:net:example.com",
            "urn:cap:net:example.com:-1",
            "urn:cap:net:[::1]:8080",
            "urn:cap:net:*",
            "urn:cap:store:read:graph:urn:x:-y",
            "urn:cap:ledger:read:my-ledger",
            "urn:cap:dev-server:read",
            "urn:cap:personal:calendar:read:freebusy",
            "urn:cap:Fs:read:-x",
            "urn:cap:-fs:read",
            "urn:cap:",
            "urn:cap",
            "urn:other:fs:read:-x",
            "",
        ];
        for scope in grants {
            assert!(!is_deny_scope(scope), "{scope} is not an exclusion");
        }
    }

    #[test]
    fn an_exclusion_is_never_granted_by_exact_match() {
        let cap = Capability::scoped(["urn:cap:fs:read:/root", "urn:cap:secret:read:-x"]);
        assert!(cap.allows("urn:cap:fs:read:/root"));
        assert!(
            !cap.allows("urn:cap:secret:read:-x"),
            "held, but an exclusion"
        );
        assert!(!cap.allows("urn:cap:fs:read:-/root"), "not held either");
        // Root is root.
        assert!(Capability::root().allows("urn:cap:secret:read:-x"));
    }

    #[test]
    fn root_attenuates_to_exactly_the_request_exclusions_included() {
        let s = ["urn:cap:fs:read:/root", "urn:cap:fs:read:-/root/secret"];
        assert_eq!(Capability::root().attenuate(s), Capability::scoped(s));
    }

    #[test]
    fn attenuation_can_add_an_exclusion_but_never_a_grant() {
        let held = Capability::scoped(["urn:cap:net:example.com"]);
        let narrowed = held.attenuate([
            "urn:cap:net:example.com",
            "urn:cap:net:-example.com/admin",
            "urn:cap:net:other.com",
        ]);
        assert_eq!(
            narrowed,
            Capability::scoped(["urn:cap:net:example.com", "urn:cap:net:-example.com/admin"])
        );
    }

    #[test]
    fn clamp_keeps_the_exclusions_of_either_side() {
        let ceiling = Capability::scoped(["urn:cap:fs:read:/root", "urn:cap:fs:read:-/root/a"]);
        let carried = Capability::scoped([
            "urn:cap:fs:read:/root",
            "urn:cap:fs:read:-/root/b",
            "urn:cap:fs:write:/root",
        ]);
        assert_eq!(
            ceiling.clamp(&carried),
            Capability::scoped([
                "urn:cap:fs:read:/root",
                "urn:cap:fs:read:-/root/a",
                "urn:cap:fs:read:-/root/b",
            ])
        );
        // The root edges are unchanged.
        assert_eq!(ceiling.clamp(&Capability::root()), ceiling);
        assert_eq!(Capability::root().clamp(&carried), carried);
    }

    /// No wire change: the serialized form is the flat scope set it always was, so a
    /// peer on the previous release (and ikigai-python / ikigai-deno) reads it unchanged.
    #[test]
    fn the_serialized_form_is_unchanged() {
        let cap = Capability::scoped(["urn:cap:fs:read:/root", "urn:cap:fs:read:-/root/secret"]);
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(
            json,
            r#"{"kind":{"Scoped":["urn:cap:fs:read:-/root/secret","urn:cap:fs:read:/root"]}}"#
        );
        assert_eq!(serde_json::from_str::<Capability>(&json).unwrap(), cap);
        assert_eq!(
            serde_json::to_string(&Capability::root()).unwrap(),
            r#"{"kind":"Root"}"#
        );
    }

    /// The scopes the property tests draw from: grants and exclusions of both real
    /// grammars, plus a grant that merely contains `-`.
    const UNIVERSE: [&str; 8] = [
        "urn:cap:fs:read:/r",
        "urn:cap:fs:read:/r/s",
        "urn:cap:fs:read:-/r/s",
        "urn:cap:fs:read:-/r/s/t",
        "urn:cap:fs:read:/r/a-b",
        "urn:cap:net:h.example",
        "urn:cap:net:-h.example/admin",
        "urn:cap:net:-other.example",
    ];

    fn subset(bits: u32) -> BTreeSet<String> {
        UNIVERSE
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, s)| s.to_string())
            .collect()
    }

    fn split(scopes: &BTreeSet<String>) -> (BTreeSet<String>, BTreeSet<String>) {
        scopes.iter().cloned().partition(|s| !is_deny_scope(s))
    }

    /// Exhaustive over every pair of subsets of the universe (65,536 pairs): one
    /// attenuate and one clamp never drop a held exclusion and never add a grant that
    /// is not held — and for clamp, not carried either.
    #[test]
    fn property_one_step_never_drops_an_exclusion_or_adds_a_grant() {
        let n = 1u32 << UNIVERSE.len();
        for a in 0..n {
            let held = subset(a);
            let (held_grants, held_denies) = split(&held);
            let cap = Capability::scoped(held.clone());
            for b in 0..n {
                let other = subset(b);
                let (other_grants, other_denies) = split(&other);
                for result in [
                    cap.attenuate(other.iter().cloned()),
                    cap.clamp(&Capability::scoped(other.clone())),
                ] {
                    let (grants, denies) = split(result.scopes().unwrap());
                    assert!(held_denies.is_subset(&denies), "{held:?} / {other:?}");
                    assert!(other_denies.is_subset(&denies), "{held:?} / {other:?}");
                    assert!(grants.is_subset(&held_grants), "{held:?} / {other:?}");
                    assert!(grants.is_subset(&other_grants), "{held:?} / {other:?}");
                }
            }
        }
    }

    /// Long random chains of attenuate and clamp (a fixed-seed xorshift, so a failure
    /// reproduces): the ORIGINAL holder's exclusions survive every step, and no grant
    /// it did not hold ever appears.
    #[test]
    fn property_no_chain_drops_an_exclusion_or_adds_a_grant() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let n = 1u64 << UNIVERSE.len();
        for _ in 0..4_000 {
            let start = subset((next() % n) as u32);
            let (start_grants, start_denies) = split(&start);
            let mut cap = Capability::scoped(start.clone());
            for _ in 0..(next() % 12) {
                let other = subset((next() % n) as u32);
                cap = if next() % 2 == 0 {
                    cap.attenuate(other)
                } else {
                    cap.clamp(&Capability::scoped(other))
                };
                let (grants, denies) = split(cap.scopes().unwrap());
                assert!(start_denies.is_subset(&denies), "{start:?} -> {cap:?}");
                assert!(grants.is_subset(&start_grants), "{start:?} -> {cap:?}");
            }
        }
    }

    /// What the property buys a module, checked against a model of the rule ikigai-fs
    /// and ikigai-http share (longest matching rule wins, an exclusion breaks a tie, no
    /// rule means no): an attenuated or clamped capability never reaches a target its
    /// parent could not.
    #[test]
    fn property_a_narrowed_capability_reaches_nothing_its_parent_could_not() {
        fn reaches(cap: &Capability, target: &str) -> bool {
            let mut best: Option<(usize, bool)> = None;
            for scope in cap.scopes().unwrap() {
                let Some(rest) = scope.strip_prefix("urn:cap:fs:read:") else {
                    continue;
                };
                let (allow, rule) = match rest.strip_prefix('-') {
                    Some(rule) => (false, rule),
                    None => (true, rest),
                };
                let covers = target
                    .strip_prefix(rule)
                    .is_some_and(|tail| tail.is_empty() || tail.starts_with('/'));
                if !covers {
                    continue;
                }
                best = match best {
                    Some((len, _)) if rule.len() < len => best,
                    Some((len, was)) if rule.len() == len => Some((len, was && allow)),
                    _ => Some((rule.len(), allow)),
                };
            }
            best.is_some_and(|(_, allow)| allow)
        }
        let targets = [
            "/r", "/r/x", "/r/s", "/r/s/x", "/r/s/t", "/r/s/t/x", "/r/a-b",
        ];
        let n = 1u32 << UNIVERSE.len();
        for a in 0..n {
            let parent = Capability::scoped(subset(a));
            for b in 0..n {
                let other = subset(b);
                for child in [
                    parent.attenuate(other.iter().cloned()),
                    parent.clamp(&Capability::scoped(other.clone())),
                ] {
                    for target in targets {
                        assert!(
                            !reaches(&child, target) || reaches(&parent, target),
                            "{child:?} reaches {target} past {parent:?}"
                        );
                    }
                }
            }
        }
    }

    // --- principals (ledger #1077) ----------------------------------------------------

    const ALICE: &str = "urn:example:person:alice";
    const BOB: &str = "urn:example:person:bob";

    fn p(iri: &str) -> String {
        principal_scope(iri).unwrap()
    }

    #[test]
    fn a_principal_scope_is_the_prefix_and_the_iri_verbatim() {
        assert_eq!(PRINCIPAL_SCOPE_PREFIX, "urn:cap:principal:");
        assert_eq!(p(ALICE), "urn:cap:principal:urn:example:person:alice");
        // The shapes hosts mint today, unescaped.
        assert_eq!(
            p("urn:example:passkey:Zx-9_a"),
            "urn:cap:principal:urn:example:passkey:Zx-9_a"
        );
        assert_eq!(
            p("https://example.org/people/brian#me"),
            "urn:cap:principal:https://example.org/people/brian#me"
        );
        // A minted scope is never deny-shaped, so narrowing reads it as a principal.
        assert!(!is_deny_scope(&p(ALICE)));
    }

    #[test]
    fn what_cannot_be_carried_verbatim_is_refused_not_escaped() {
        for bad in [
            "",
            "alice",
            "two words",
            "urn:x:two words",
            "urn:x:<a>",
            "*",
        ] {
            assert!(
                matches!(principal_scope(bad), Err(PrincipalError::NotAnIri { .. })),
                "{bad:?}"
            );
        }
        for wild in ["urn:x:*", "urn:*:x", "https://e.org/*"] {
            assert_eq!(
                principal_scope(wild),
                Err(PrincipalError::Wildcard {
                    offered: wild.to_string()
                })
            );
        }
        for deny in ["x:-y", "urn:-x"] {
            assert_eq!(
                principal_scope(deny),
                Err(PrincipalError::DenyShaped {
                    offered: deny.to_string()
                })
            );
        }
        let long = format!("urn:x:{}", "a".repeat(MAX_PRINCIPAL_LEN));
        assert_eq!(
            principal_scope(&long),
            Err(PrincipalError::TooLong { length: long.len() })
        );
        let longest = format!("urn:x:{}", "a".repeat(MAX_PRINCIPAL_LEN - 6));
        assert!(principal_scope(&longest).is_ok());
    }

    #[test]
    fn a_capability_with_no_principal_reads_as_none() {
        let cap = Capability::scoped(["urn:cap:fs:read:/r"]);
        assert_eq!(cap.principal(), None);
        assert!(!cap.acts_as(ALICE));
        assert_eq!(Capability::scoped(Vec::<String>::new()).principal(), None);
    }

    #[test]
    fn a_minted_principal_reads_back_and_is_granted() {
        let cap = Capability::scoped(["urn:cap:fs:read:/r"])
            .with_principal(ALICE)
            .unwrap();
        assert_eq!(cap.principal(), Some(ALICE));
        assert!(cap.acts_as(ALICE));
        assert!(!cap.acts_as(BOB));
        assert!(cap.allows(&p(ALICE)));
        assert!(!cap.allows(&p(BOB)));
        assert!(
            cap.allows("urn:cap:fs:read:/r"),
            "the authority is unchanged"
        );
        // Minting again REPLACES: one principal per capability.
        let re = cap.with_principal(BOB).unwrap();
        assert_eq!(re.principal(), Some(BOB));
        assert_eq!(
            re.scopes().unwrap().len(),
            2,
            "alice is gone, not kept beside bob: {re:?}"
        );
        // A refused IRI leaves nothing minted.
        assert!(cap.with_principal("not an iri").is_err());
    }

    #[test]
    fn several_principals_are_none() {
        let both = Capability::scoped([p(ALICE), p(BOB), "urn:cap:x".to_string()]);
        assert_eq!(both.principal(), None);
        assert!(!both.acts_as(ALICE) && !both.acts_as(BOB));
        assert!(!both.allows(&p(ALICE)) && !both.allows(&p(BOB)));
        assert!(both.allows("urn:cap:x"), "only the names are void");
        // An ambiguous capability cannot be narrowed into one of its names...
        for narrowed in [
            both.attenuate([p(BOB)]),
            both.attenuate([p(ALICE), "urn:cap:x".to_string()]),
            both.clamp(&Capability::scoped([p(BOB)])),
        ] {
            assert_eq!(narrowed.principal(), None, "{narrowed:?}");
        }
        // ...and root narrowed to several yields none of them.
        let from_root = Capability::root().attenuate([p(ALICE), p(BOB), "urn:cap:x".to_string()]);
        assert_eq!(from_root, Capability::scoped(["urn:cap:x"]));
    }

    #[test]
    fn attenuation_never_adds_a_principal() {
        let anon = Capability::scoped(["urn:cap:x"]);
        let claimed = anon.attenuate(["urn:cap:x".to_string(), p(ALICE)]);
        assert_eq!(claimed, Capability::scoped(["urn:cap:x"]));
        assert_eq!(claimed.principal(), None);
    }

    #[test]
    fn attenuation_never_changes_a_principal() {
        let alice = Capability::scoped(["urn:cap:x"])
            .with_principal(ALICE)
            .unwrap();
        // Naming someone else: alice is not asked for, so she is shed; bob never enters.
        let as_bob = alice.attenuate(["urn:cap:x".to_string(), p(BOB)]);
        assert_eq!(as_bob, Capability::scoped(["urn:cap:x"]));
        // Naming both keeps alice alone.
        let both = alice.attenuate(["urn:cap:x".to_string(), p(ALICE), p(BOB)]);
        assert_eq!(both.principal(), Some(ALICE));
        assert!(!both.allows(&p(BOB)));
        // Shedding is narrowing: the sub-request goes out anonymous.
        assert_eq!(alice.attenuate(["urn:cap:x"]).principal(), None);
    }

    /// Ledger #879's failure, reproduced against the convention: a door's session names
    /// alice and the client carries a narrower capability of its own (`ikigai mcp
    /// --grant`). The clamp keeps the door's principal; under plain intersection the
    /// request would arrive anonymous.
    #[test]
    fn clamp_keeps_the_ceilings_principal_and_never_the_carried_one() {
        let session = Capability::scoped(["urn:cap:a", "urn:cap:b"])
            .with_principal(ALICE)
            .unwrap();
        // A narrower carried capability that does not name her.
        let narrower = session.clamp(&Capability::scoped(["urn:cap:a"]));
        assert_eq!(narrower.principal(), Some(ALICE));
        assert!(narrower.allows("urn:cap:a") && !narrower.allows("urn:cap:b"));
        // A carried capability naming someone else.
        let forged = session.clamp(&Capability::scoped(["urn:cap:a".to_string(), p(BOB)]));
        assert_eq!(forged.principal(), Some(ALICE));
        assert!(!forged.allows(&p(BOB)));
        // Carried root: the ceiling, name and all.
        assert_eq!(session.clamp(&Capability::root()), session);
        // An anonymous ceiling never takes a carried name.
        let anon = Capability::scoped(["urn:cap:a"]);
        assert_eq!(
            anon.clamp(&Capability::scoped(["urn:cap:a".to_string(), p(BOB)]))
                .principal(),
            None
        );
    }

    #[test]
    fn root_holds_every_principal_and_names_none() {
        let root = Capability::root();
        assert_eq!(root.principal(), None);
        assert!(root.acts_as(ALICE) && root.acts_as(BOB));
        assert!(root.allows(&p(ALICE)));
        // Root narrows to any one principal: that is how a root-holding host mints.
        let alice = root.attenuate(["urn:cap:x".to_string(), p(ALICE)]);
        assert_eq!(alice.principal(), Some(ALICE));
        // A root ceiling trusts the carried capability, name included.
        let carried = Capability::scoped(["urn:cap:x".to_string(), p(BOB)]);
        assert_eq!(root.clamp(&carried).principal(), Some(BOB));
        // Root cannot carry one name.
        assert_eq!(root.with_principal(ALICE), Err(PrincipalError::Root));
    }

    #[test]
    fn a_wildcard_is_never_an_identity() {
        for wild in [
            "urn:cap:*",
            "urn:cap:principal:*",
            "urn:cap:principal:urn:*",
        ] {
            let cap = Capability::scoped([wild]);
            assert_eq!(cap.principal(), None, "{wild}");
            assert!(!cap.acts_as(ALICE), "{wild}");
            assert!(!cap.allows(&p(ALICE)), "{wild}");
            // Nor does it make a real principal beside it ambiguous.
            let beside = Capability::scoped([wild.to_string(), p(ALICE)]);
            assert_eq!(beside.principal(), Some(ALICE), "{wild}");
            // And narrowing cannot turn it into one.
            assert_eq!(cap.attenuate([p(ALICE)]).principal(), None, "{wild}");
            assert_eq!(cap.clamp(&Capability::scoped([p(ALICE)])).principal(), None);
        }
        // The declared presence test is satisfied by a held wildcard: that is why an
        // endpoint asking "who" reads `principal()`, which says nobody.
        let presence = Capability::scoped(["urn:cap:principal:*"]);
        assert!(crate::select::cap_satisfies(
            &presence,
            "urn:cap:principal:*"
        ));
        assert_eq!(presence.principal(), None);
    }

    #[test]
    fn a_principal_travels_the_wire_in_the_unchanged_form() {
        let cap = Capability::scoped(["urn:cap:x"])
            .with_principal(ALICE)
            .unwrap();
        let json = serde_json::to_string(&cap).unwrap();
        assert_eq!(
            json,
            r#"{"kind":{"Scoped":["urn:cap:principal:urn:example:person:alice","urn:cap:x"]}}"#
        );
        let back: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(back.principal(), Some(ALICE));
    }

    /// The universe the principal properties draw from: two names, both wildcard shapes,
    /// an unrelated grant and an exclusion.
    const PRINCIPAL_UNIVERSE: [&str; 7] = [
        "urn:cap:principal:urn:example:person:alice",
        "urn:cap:principal:urn:example:person:bob",
        "urn:cap:principal:*",
        "urn:cap:*",
        "urn:cap:fs:read:/r",
        "urn:cap:fs:read:-/r/s",
        "urn:cap:net:h.example",
    ];

    fn principal_subset(bits: u32) -> BTreeSet<String> {
        PRINCIPAL_UNIVERSE
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, s)| s.to_string())
            .collect()
    }

    /// Every pair of subsets: an attenuation's principal is the holder's or none; a
    /// clamp's is exactly the ceiling's; and a capability grants a principal scope
    /// exactly when it names that principal.
    #[test]
    fn property_narrowing_never_adds_or_changes_a_principal() {
        let n = 1u32 << PRINCIPAL_UNIVERSE.len();
        let grants_exactly_its_principal = |cap: &Capability| {
            for who in [ALICE, BOB] {
                assert_eq!(
                    cap.allows(&p(who)),
                    cap.principal() == Some(who),
                    "{cap:?} / {who}"
                );
                assert_eq!(cap.acts_as(who), cap.principal() == Some(who), "{cap:?}");
            }
        };
        for a in 0..n {
            let held = Capability::scoped(principal_subset(a));
            grants_exactly_its_principal(&held);
            for b in 0..n {
                let other = principal_subset(b);
                let attenuated = held.attenuate(other.iter().cloned());
                assert!(
                    attenuated.principal().is_none() || attenuated.principal() == held.principal(),
                    "{held:?} / {other:?} -> {attenuated:?}"
                );
                grants_exactly_its_principal(&attenuated);
                let clamped = held.clamp(&Capability::scoped(other.clone()));
                assert_eq!(
                    clamped.principal(),
                    held.principal(),
                    "{held:?} / {other:?} -> {clamped:?}"
                );
                grants_exactly_its_principal(&clamped);
            }
        }
    }

    /// Long random chains of attenuate and clamp from a capability naming alice (or
    /// nobody): the name is only ever alice or none, never bob.
    #[test]
    fn property_no_chain_changes_a_principal() {
        let mut state: u64 = 0xD1B5_4A32_D192_ED03;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let n = 1u64 << PRINCIPAL_UNIVERSE.len();
        for start in [
            Capability::scoped(["urn:cap:fs:read:/r"]),
            Capability::scoped(["urn:cap:fs:read:/r"])
                .with_principal(ALICE)
                .unwrap(),
        ] {
            for _ in 0..2_000 {
                let mut cap = start.clone();
                for _ in 0..(next() % 12) {
                    let other = principal_subset((next() % n) as u32);
                    cap = if next() % 2 == 0 {
                        cap.attenuate(other)
                    } else {
                        cap.clamp(&Capability::scoped(other))
                    };
                    assert!(
                        cap.principal().is_none() || cap.principal() == start.principal(),
                        "{start:?} -> {cap:?}"
                    );
                }
            }
        }
    }
}
