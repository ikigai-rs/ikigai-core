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
            Kind::Root => requested,
            Kind::Scoped(held) => {
                let mut kept: BTreeSet<String> = held
                    .iter()
                    .filter(|scope| is_deny_scope(scope) || requested.contains(*scope))
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
    pub fn clamp(&self, carried: &Capability) -> Capability {
        match carried.scopes() {
            // The peer carried root — clamp to our own ceiling.
            None => self.clone(),
            // Keep only grants the ceiling already holds, and every exclusion.
            Some(scopes) => self.attenuate(scopes.iter().cloned()),
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
    /// What is still absent is INFIX matching over the hierarchy — `urn:cap:personal:*:read`
    /// covering `urn:cap:personal:calendar:read:detail`. That can be added later without
    /// changing any tokens.
    pub fn allows(&self, scope: &str) -> bool {
        match &self.kind {
            Kind::Root => true,
            Kind::Scoped(held) => held.contains(scope) && !is_deny_scope(scope),
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
}
