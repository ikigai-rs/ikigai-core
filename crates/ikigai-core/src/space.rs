use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::endpoint::{Endpoint, Invocation};
use crate::error::{Error, Result};
use crate::grammar::{Bindings, Grammar};
use crate::iri::Iri;
use crate::kernel::Clock;
use crate::repr::{Representation, Time};
use crate::request::Request;
use crate::topology::{SpaceKind, Topology};

/// The resolution chain a request is resolved in: the corridors a host injected
/// ahead of the kernel's root space, and whether the root is in the chain at all.
///
/// A kernel resolves a request against
/// `⟨injected corridors (innermost first), root⟩`. Every sub-request an endpoint
/// issues inherits the chain, so what is true of the request is true of
/// everything it gives rise to. Two faces of that one mechanism:
///
/// - **Injection** — [`with_named`](Self::with_named) puts a space ahead of the
///   root, where it can **shadow** a root door for this request and all of its
///   sub-requests. This is how a host composes per-request context (a temporal
///   corridor pinning `urn:time:now`, say) without rebuilding its space tree.
///   Reachable only through [`Kernel::issue_in`](crate::Kernel::issue_in), i.e.
///   only by whoever holds the kernel: an injected corridor placed innermost can
///   stand in for anything, so injecting is authority.
/// - **Severing** — [`confined`](Self::confined) cuts the root off. Inside a
///   severed chain a resource outside it is
///   [`Unresolved`](crate::Error::Unresolved), never
///   [`Denied`](crate::Error::Denied): a denial is a decision that can be
///   misconfigured, an unresolvable identifier has nowhere to go. This is the
///   only face an endpoint can reach ([`Invocation::confine`](crate::Invocation::confine),
///   [`Confine`](crate::Confine)), and it is strictly narrowing.
///
/// [`empty`](Self::empty) — nothing injected, root present — is what every
/// [`Kernel::issue`](crate::Kernel::issue) resolves in, and it is the status quo:
/// its [`fingerprint`](Self::fingerprint) is `0`, so today's cache entries and
/// today's cache keys are untouched.
///
/// The chain is the resolution context **everywhere** the kernel answers a
/// question about resolution, not only on the issue path: selection
/// ([`Kernel::select_action_in`](crate::Kernel::select_action_in) and the
/// invocation forms) offers what the chain can resolve, the cache probe
/// ([`Kernel::is_cached_in`](crate::Kernel::is_cached_in)) answers for the
/// chain's entry, and a pipe stage can run in it
/// ([`Kernel::issue_with_incoming_in`](crate::Kernel::issue_with_incoming_in)).
/// A chain may also carry a **clock**, derived from a temporal corridor at
/// injection ([`with_named_at`](Self::with_named_at)), which
/// [`Invocation::now`](crate::Invocation::now) prefers — so pinning time pins
/// both the resolved `urn:time:now` and the clock an endpoint reads.
///
/// The kernel reserves `urn:kernel:*` ahead of the chain: no corridor can shadow
/// a kernel operation, and a severed chain still reaches them (capability-gated,
/// as always).
///
/// # A corridor's name is a claim
///
/// The representation cache keys on this chain's fingerprint, and the fingerprint
/// is computed over the corridors' **names**, not their contents or their
/// addresses. So `with_named(n, s)` asserts: *any corridor named `n` holds the
/// same doors as `s`.* Same name ⇒ same doors ⇒ same resource — exactly the
/// contract [`Resolved::canonical`] makes for a rewritten name. Name two
/// different corridors alike and one request is served the other's cached answer;
/// the flip side is what makes naming worth it: a corridor rebuilt per request
/// under the same name shares one cache entry across every request that names it.
/// (An [anonymous](Self::with) corridor shares with nothing but its own clones.)
///
/// ```
/// use std::sync::Arc;
/// use futures::executor::block_on;
/// use ikigai_core::{
///     Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation,
///     Request, Scope, Verb,
/// };
///
/// let pinned = || {
///     Arc::new(EndpointSpace::new().bind(
///         Exact::new("urn:time:now"),
///         FnEndpoint::new("pinned", |_| {
///             Ok(Representation::new(ReprType::new("text/plain"), b"18:00Z".to_vec()).cacheable())
///         }),
///     ))
/// };
/// let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
/// let cap = Capability::root();
/// let now = || Request::new(Verb::Source, Iri::parse("urn:time:now").unwrap());
/// let name = || Iri::parse("urn:ctx:time:2026-09-25T18:00Z").unwrap();
///
/// // Two requests, two freshly built corridors, ONE name: one cache entry.
/// block_on(kernel.issue_in(now(), &cap, Scope::empty().with_named(name(), pinned()))).unwrap();
/// block_on(kernel.issue_in(now(), &cap, Scope::empty().with_named(name(), pinned()))).unwrap();
/// assert_eq!(kernel.cache_len(), 1);
///
/// // A different name is a different chain, so a different entry — and the
/// // empty chain never sees either: `urn:time:now` is not bound in the root.
/// let other = Iri::parse("urn:ctx:time:2026-09-26T09:00Z").unwrap();
/// block_on(kernel.issue_in(now(), &cap, Scope::empty().with_named(other, pinned()))).unwrap();
/// assert_eq!(kernel.cache_len(), 2);
/// assert!(block_on(kernel.issue(now(), &cap)).is_err());
/// ```
#[derive(Clone, Default)]
pub struct Scope {
    /// `None` IS the empty chain. The chain lives behind an `Arc` so that the
    /// scope every `issue` carries, clones into its invocation and hands to each
    /// sub-request is one word: the empty chain costs a null check, a non-empty
    /// one a refcount bump. Building a chain is cold; carrying it is the hot path.
    chain: Option<Arc<Chain>>,
}

/// A non-empty chain: what was injected, under what identity, and whether the
/// root is still on the end of it.
#[derive(Clone)]
struct Chain {
    /// The injected corridors, **outermost first** (the most recently injected is
    /// last, and is consulted first). Parallel to `identities`.
    injected: Vec<Arc<dyn Space>>,
    /// Each injected corridor's identity, in `injected`'s order.
    identities: Vec<CorridorIdentity>,
    /// `true` when the root has been cut off the chain.
    severed: bool,
    /// The chain's fingerprint, computed once when the chain is built so reading
    /// it costs nothing on the issue path.
    fingerprint: u64,
    /// The chain's clock: the one a temporal corridor DERIVED at injection
    /// ([`Scope::with_named_at`]), read by [`Invocation::now`](crate::Invocation::now)
    /// ahead of the issuer's. Innermost wins; a corridor injected without one leaves
    /// it as it was; confinement keeps it. **Not fingerprinted** — the clock is a
    /// property of the corridor's name (see `with_named_at` on why), and the kernel
    /// never reads it for validity.
    clock: Option<Arc<dyn Clock>>,
}

/// What a corridor is fingerprinted by: the name its injector claimed for it, or
/// — with no name — a process-unique number, so it shares a cache entry with its
/// own clones and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CorridorIdentity {
    Named(Iri),
    Anonymous(u64),
}

/// Source of anonymous corridor identities. A counter rather than the `Arc`'s
/// address because an address is reused once the corridor is dropped, while a
/// cache entry keyed on it lives on — a later, unrelated corridor at the same
/// address would be served the first one's answers.
static ANONYMOUS_CORRIDORS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The identity a corridor is injected under when an injector names it: the
/// injector's `name`, which must agree with the identity the space claims for
/// itself if it claims one. A disagreement is refused — see
/// [`Scope::with_named`] for why neither name may quietly win.
fn claim(name: Iri, space: &Arc<dyn Space>) -> CorridorIdentity {
    check_claim(&name, space);
    CorridorIdentity::Named(name)
}

/// Refuse a `name` for `space` that disagrees with the identity the space claims
/// for itself. Shared by every site that names a corridor — injection,
/// confinement, [`Confine::new`](crate::Confine::new).
pub(crate) fn check_claim(name: &Iri, space: &Arc<dyn Space>) {
    if let Some(own) = space.id() {
        assert!(
            own == *name,
            "a self-named space is injected under its own identity: the space claims \
             `{own}` and the injector named it `{name}` — inject it with `Scope::with` \
             or name it as it names itself",
        );
    }
}

impl Scope {
    /// The empty chain: nothing injected, root present. Its fingerprint is `0`.
    pub fn empty() -> Self {
        Scope::default()
    }

    /// Inject a corridor ahead of everything already injected (innermost), under
    /// the identity the space itself claims ([`Space::id`]) — or **anonymously**
    /// when it claims none. An anonymous corridor is fingerprinted by a fresh
    /// process-unique identity, so a request resolved through it shares a cache
    /// entry only with requests carrying a *clone* of this very scope — a corridor
    /// rebuilt per request never shares with the last one. Sound, and useless for
    /// the case injection exists for; name the space (`.named(iri)`) or use
    /// [`with_named`](Self::with_named).
    pub fn with(self, space: Arc<dyn Space>) -> Self {
        let identity = match space.id() {
            Some(id) => CorridorIdentity::Named(id),
            None => CorridorIdentity::Anonymous(
                ANONYMOUS_CORRIDORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ),
        };
        self.edit(|chain| {
            chain.injected.push(space);
            chain.identities.push(identity);
        })
    }

    /// Inject a **named** corridor ahead of everything already injected
    /// (innermost). The name is the corridor's identity for the cache — see the
    /// type-level note on what naming claims.
    ///
    /// # A self-named space is not renamed here
    ///
    /// A space that claims its own identity ([`Space::id`], set by `.named(iri)`)
    /// is injected under that identity; `name` must agree with it, and a
    /// different `name` is **refused** — this method panics. The alternative
    /// readings are both the bug identity exists to close: taking the injector's
    /// name over the space's puts one set of doors under two names (two cache
    /// partitions for one corridor), and taking the space's over the injector's
    /// silently drops whatever the injector's name carried (a temporal corridor
    /// named for its instant, injected as one space named for its kind, would
    /// serve every instant one cached answer). An injector holding a self-named
    /// space uses [`with`](Self::with) and lets the space say who it is. Every
    /// space this crate shipped before 0.1.78 claims no id, so nothing existing
    /// reaches the refusal.
    ///
    /// ```should_panic
    /// use std::sync::Arc;
    /// use ikigai_core::{EndpointSpace, Iri, Scope};
    ///
    /// let space = Arc::new(EndpointSpace::new().named(Iri::parse("urn:example:space:a").unwrap()));
    /// // Refused: the space says `urn:example:space:a`, the injector says otherwise.
    /// let _ = Scope::empty().with_named(Iri::parse("urn:example:space:b").unwrap(), space);
    /// ```
    pub fn with_named(self, name: Iri, space: Arc<dyn Space>) -> Self {
        let identity = claim(name, &space);
        self.edit(|chain| {
            chain.injected.push(space);
            chain.identities.push(identity);
        })
    }

    /// Inject a **named temporal corridor**: a corridor that binds the time name
    /// (`urn:time:now`, say) to one instant, AND the [`Clock`] derived from that
    /// instant, in one call — so an endpoint under it sees the same pinned time
    /// whether it *resolves* time or calls [`Invocation::now`](crate::Invocation::now).
    /// The two cannot be set independently: there is no way to put a clock on a
    /// chain except beside the corridor it is derived from (ledger #517).
    ///
    /// # The pairing is a claim the injector makes
    ///
    /// Core cannot verify that the corridor's time door answers the instant the
    /// clock reads — it would have to resolve the door to find out, and a corridor
    /// is any [`Space`]. So `with_named_at(n, s, c)` asserts, beside what
    /// [`with_named`](Self::with_named) already asserts about `n` and `s`: *the time
    /// `s` binds under `n` is the time `c` reads.* That is the same shape as a
    /// corridor's name being a claim, and it is why the clock is **not** part of
    /// the [fingerprint](Self::fingerprint): the clock is a property of the name.
    /// Two injections under one name with different clocks are one claim made
    /// twice with different content — the injector's error, exactly as two
    /// different spaces under one name would be — and the cache treats them as one
    /// context. (The alternative, fingerprinting the clock, has nothing to hash: an
    /// `Arc<dyn Clock>` has no stable identity, and the instant exists only for a
    /// clock that does not move.)
    ///
    /// At most one clock per chain, and the **innermost** wins: a temporal corridor
    /// injected inside another replaces its clock, as its `urn:time:now` shadows the
    /// outer one's. A corridor injected without a clock leaves the chain's as it
    /// was, and [`confined`](Self::confined) keeps it — an endpoint cannot change
    /// the host's time any more than it can drop the host's corridors.
    ///
    /// The kernel keeps its **own** clock for validity: an [`Expiry::At`](crate::Expiry)
    /// deadline is judged against [`Kernel::with_clock`](crate::Kernel::with_clock),
    /// never against this one, so a pinned past cannot un-expire a live entry and a
    /// pinned future cannot expire a fresh one. What this clock changes is what an
    /// endpoint *computes*; what it never changes is what the cache *serves*.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use futures::executor::block_on;
    /// use ikigai_core::{
    ///     AsyncFnEndpoint, Capability, EndpointSpace, Exact, Expiry, FixedClock, FnEndpoint, Iri,
    ///     Kernel, ReprType, Representation, Request, Scope, Verb,
    /// };
    ///
    /// fn text(s: String) -> Representation {
    ///     Representation::new(ReprType::new("text/plain"), s.into_bytes())
    /// }
    /// // An endpoint that reads time BOTH ways, and is cacheable on its own account.
    /// let stamp = AsyncFnEndpoint::new("stamp", move |inv| {
    ///     Box::pin(async move {
    ///         let resolved = inv.source(&Iri::parse("urn:time:now").unwrap()).await?;
    ///         let clock = inv.now().map(|t| t.as_millis()).unwrap_or(0);
    ///         Ok(text(format!("{} {clock}", String::from_utf8_lossy(&resolved.bytes))).cacheable())
    ///     })
    /// });
    /// // The root's `urn:time:now` is live: the kernel's clock, uncacheable.
    /// let kernel = Kernel::new(Arc::new(
    ///     EndpointSpace::new()
    ///         .bind(
    ///             Exact::new("urn:time:now"),
    ///             FnEndpoint::new("now", |inv| Ok(text(inv.now().unwrap().as_millis().to_string()))),
    ///         )
    ///         .bind_arc(Exact::new("urn:stamp"), Arc::new(stamp)),
    /// ))
    /// .with_clock(Arc::new(FixedClock::at(2_000)));
    /// let cap = Capability::root();
    /// let stamp = || Request::new(Verb::Source, Iri::parse("urn:stamp").unwrap());
    ///
    /// // The corridor pins BOTH: the door it binds, and the clock derived from it.
    /// let at_six = || {
    ///     Scope::empty().with_named_at(
    ///         Iri::parse("urn:ctx:time:2026-09-25T18:00Z").unwrap(),
    ///         Arc::new(EndpointSpace::new().bind(
    ///             Exact::new("urn:time:now"),
    ///             FnEndpoint::new("six", |_| Ok(text("1000".into()).cacheable())),
    ///         )),
    ///         Arc::new(FixedClock::at(1_000)),
    ///     )
    /// };
    /// let live = block_on(kernel.issue(stamp(), &cap)).unwrap();
    /// let pinned = block_on(kernel.issue_in(stamp(), &cap, at_six())).unwrap();
    /// assert_eq!(live.bytes, b"2000 2000");
    /// assert_eq!(pinned.bytes, b"1000 1000");
    ///
    /// // Pinning the context turns an uncacheable "now" into an immutable "then".
    /// assert_eq!(live.expiry, Expiry::Always);
    /// assert_eq!(pinned.expiry, Expiry::Never);
    /// assert!(kernel.is_cached_in(&stamp(), &cap, &at_six()));
    /// ```
    ///
    /// A self-named space is injected under its own identity and `name` must agree
    /// with it, as for [`with_named`](Self::with_named).
    pub fn with_named_at(self, name: Iri, space: Arc<dyn Space>, clock: Arc<dyn Clock>) -> Self {
        let identity = claim(name, &space);
        self.edit(|chain| {
            chain.injected.push(space);
            chain.identities.push(identity);
            chain.clock = Some(clock);
        })
    }

    /// The chain's clock — the one its innermost temporal corridor derived
    /// ([`with_named_at`](Self::with_named_at)) — or `None` when no corridor in it
    /// pinned time. Read by [`Invocation::now`](crate::Invocation::now) ahead of
    /// the issuer's clock; never by the kernel for validity.
    pub fn clock(&self) -> Option<&Arc<dyn Clock>> {
        self.chain.as_ref().and_then(|chain| chain.clock.as_ref())
    }

    /// The chain's "now", per its [clock](Self::clock); `None` when it has none.
    pub fn now(&self) -> Option<Time> {
        self.clock().map(|clock| clock.now())
    }

    /// Cut the root off the chain: a request resolved in the result reaches only
    /// the injected corridors, and anything else is
    /// [`Unresolved`](crate::Error::Unresolved). Idempotent.
    pub fn sever(self) -> Self {
        self.edit(|chain| chain.severed = true)
    }

    /// The chain an endpoint runs in once confined to `space`: everything already
    /// injected, then `space` **behind** it — where the root used to be — and no
    /// root. The one chain-changing operation an endpoint can reach
    /// ([`Invocation::confine`](crate::Invocation::confine)), and its placement
    /// is the whole authority argument: a confining endpoint can only ever put a
    /// space where the root was and cut the root off. It cannot get ahead of a
    /// corridor the host injected, so it can never shadow one; and the root's
    /// doors are exactly what confinement exists to remove. Relative to the chain
    /// it started in, nothing resolves differently except what the root would
    /// have answered.
    ///
    /// A self-named `space` is the corridor under its own identity and `name` must
    /// agree with it, as for [`with_named`](Self::with_named).
    pub fn confined(self, name: Iri, space: Arc<dyn Space>) -> Self {
        let identity = claim(name, &space);
        self.edit(|chain| {
            chain.injected.insert(0, space);
            chain.identities.insert(0, identity);
            chain.severed = true;
        })
    }

    /// Apply a builder step: unshare (or start) the chain, edit it, refingerprint.
    fn edit(self, step: impl FnOnce(&mut Chain)) -> Self {
        let mut chain = match self.chain {
            None => Chain {
                injected: Vec::new(),
                identities: Vec::new(),
                severed: false,
                fingerprint: 0,
                clock: None,
            },
            Some(shared) => Arc::try_unwrap(shared).unwrap_or_else(|shared| (*shared).clone()),
        };
        step(&mut chain);
        chain.refingerprint();
        Scope {
            chain: Some(Arc::new(chain)),
        }
    }

    /// The injected corridors, outermost first (the most recently injected is
    /// last, and is consulted first).
    pub fn spaces(&self) -> &[Arc<dyn Space>] {
        self.chain.as_ref().map_or(&[], |chain| &chain.injected)
    }

    /// Whether the root has been cut off the chain.
    pub fn is_severed(&self) -> bool {
        self.chain.as_ref().is_some_and(|chain| chain.severed)
    }

    /// Whether this is the empty chain — nothing injected, root present — the
    /// one every plain [`Kernel::issue`](crate::Kernel::issue) resolves in.
    pub fn is_empty(&self) -> bool {
        match self.chain.as_ref() {
            None => true,
            Some(chain) => chain.is_empty(),
        }
    }

    /// The chain's fingerprint: the dimension the representation cache keys on
    /// beside the request id and the capability. `0` for the empty chain, so a
    /// [`CacheKey`](crate::CacheKey) built without a scope is the empty chain's
    /// key. Otherwise BLAKE3 over whether the root is present and each corridor's
    /// identity in chain order — the **whole** chain, not the corridors actually
    /// consulted, which is sound and over-partitioned (two chains differing only
    /// in a corridor neither request touched do not share).
    pub fn fingerprint(&self) -> u64 {
        self.chain.as_ref().map_or(0, |chain| chain.fingerprint)
    }

    /// Every space the chain consults, in the order it consults them: each
    /// injected corridor innermost first, then `root` unless the chain is severed.
    /// **The one walk** — resolution ([`resolve_in`](Self::resolve_in)) and
    /// selection ([`view`](Self::view)) both take their order from here, so what
    /// the manifold offers inside a chain is what resolution in that chain reaches.
    pub(crate) fn consulted<'a>(
        &'a self,
        root: &'a Arc<dyn Space>,
    ) -> impl Iterator<Item = &'a Arc<dyn Space>> + 'a {
        let (corridors, severed) = match self.chain.as_ref() {
            None => (&[][..], false),
            Some(chain) => (chain.injected.as_slice(), chain.severed),
        };
        corridors.iter().rev().chain((!severed).then_some(root))
    }

    /// Resolve `request` against the chain: the first hit along
    /// [`consulted`](Self::consulted), else a miss. A hit from a **named**
    /// corridor whose space did not name an answerer itself is reported as
    /// answered by the corridor ([`Resolved::answered_by`]): the corridor's name
    /// is its identity in this chain, and it is what a cache keyed on the
    /// corridors actually consulted would key on.
    pub(crate) fn resolve_in(&self, request: &Request, root: &Arc<dyn Space>) -> Resolution {
        // The empty chain is the hot path — every plain `issue` — and is one null
        // check straight to the root; the walk below is the general case.
        let Some(chain) = self.chain.as_ref() else {
            return root.resolve(request, self);
        };
        let corridors = chain.injected.iter().zip(chain.identities.iter()).rev();
        for (space, identity) in corridors {
            if let Resolution::Hit(resolved) = space.resolve(request, self) {
                return Resolution::Hit(match identity {
                    CorridorIdentity::Named(name) => resolved.with_answered_by(name.clone()),
                    CorridorIdentity::Anonymous(_) => resolved,
                });
            }
        }
        if chain.severed {
            return Resolution::Miss;
        }
        root.resolve(request, self)
    }

    /// The arrangement this chain sees, as a tree: a [`Chain`](SpaceKind::Chain)
    /// node whose layers are the injected corridors innermost first, then `root`
    /// unless the chain is severed — the order [`consulted`](Self::consulted)
    /// walks and the fingerprint hashes. A corridor injected under a name it did
    /// not claim itself is reported under that name; an anonymous one is left for
    /// the renderer to skolemize. The chain node is `urn:ikigai:chain:root` for
    /// the empty chain and `urn:ikigai:chain:{fingerprint}` (sixteen hex digits, as
    /// `urn:kernel:cache` prints it) otherwise, so two chains' graphs can share a
    /// store without their entry points colliding.
    pub(crate) fn topology(&self, root: &Arc<dyn Space>) -> Topology {
        let id = match self.fingerprint() {
            0 => "urn:ikigai:chain:root".to_string(),
            fingerprint => format!("urn:ikigai:chain:{fingerprint:016x}"),
        };
        let mut node = Topology::new(SpaceKind::Chain {
            severed: self.is_severed(),
        })
        .with_id(Iri::parse(id).ok());
        if let Some(chain) = self.chain.as_ref() {
            for (space, identity) in chain.injected.iter().zip(chain.identities.iter()).rev() {
                let mut layer = space.topology();
                if let (None, CorridorIdentity::Named(name)) = (&layer.id, identity) {
                    layer.id = Some(name.clone());
                }
                node = node.child(layer);
            }
        }
        if !self.is_severed() {
            node = node.child(root.topology());
        }
        node
    }

    /// The chain as ONE enumerable space, for selection: `ahead` first (the
    /// kernel's own operations, which no corridor can shadow), then everything
    /// [`consulted`](Self::consulted). A pattern bound in more than one member is
    /// listed once, as the member consulted first binds it — the manifold names
    /// what resolution would reach, and a shadowed door is not reachable.
    pub(crate) fn view(&self, ahead: Option<Arc<dyn Space>>, root: &Arc<dyn Space>) -> ChainView {
        let mut spaces: Vec<Arc<dyn Space>> = ahead.into_iter().collect();
        spaces.extend(self.consulted(root).cloned());
        ChainView { spaces }
    }
}

/// A resolution chain seen as one space — first hit wins, entries deduplicated
/// by pattern, first binding wins — so the selection walks (`entries → Meta →
/// describe`) can run over a chain exactly as they run over a root. Built by
/// [`Scope::view`]; never bound in a tree.
pub(crate) struct ChainView {
    spaces: Vec<Arc<dyn Space>>,
}

impl Space for ChainView {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        for space in &self.spaces {
            if let Resolution::Hit(resolved) = space.resolve(request, scope) {
                return Resolution::Hit(resolved);
            }
        }
        Resolution::Miss
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut enumerable = false;
        for space in &self.spaces {
            if let Some(inner) = space.entries() {
                enumerable = true;
                entries.extend(
                    inner
                        .into_iter()
                        .filter(|entry| seen.insert(entry.pattern.clone())),
                );
            }
        }
        enumerable.then_some(entries)
    }
}

impl Chain {
    fn is_empty(&self) -> bool {
        self.injected.is_empty() && !self.severed
    }

    fn refingerprint(&mut self) {
        if self.is_empty() {
            self.fingerprint = 0;
            return;
        }
        let mut hasher = blake3::Hasher::new();
        crate::hashing::feed_u8(&mut hasher, if self.severed { 2 } else { 1 });
        for identity in &self.identities {
            match identity {
                CorridorIdentity::Named(name) => {
                    crate::hashing::feed_u8(&mut hasher, 1);
                    crate::hashing::feed_str(&mut hasher, name.as_str());
                }
                CorridorIdentity::Anonymous(id) => {
                    crate::hashing::feed_u8(&mut hasher, 2);
                    hasher.update(&id.to_le_bytes());
                }
            }
        }
        let digest = hasher.finalize();
        self.fingerprint = u64::from_le_bytes(digest.as_bytes()[..8].try_into().expect("8 bytes"));
    }
}

/// The chain as text, innermost first, ending in `root` or `severed`: e.g.
/// `urn:ctx:doc:7 root`, or `urn:ctx:doc:7 severed`. An anonymous corridor
/// renders as `_:<n>` — a blank node, which is what a space without a name is.
/// This is what the kernel puts on a trace event under
/// [`SCOPE_NOTE`](crate::SCOPE_NOTE); the two terminal tokens carry no colon, so
/// they can never be confused with a corridor's IRI.
impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some(chain) = self.chain.as_ref() else {
            return f.write_str("root");
        };
        for identity in chain.identities.iter().rev() {
            match identity {
                CorridorIdentity::Named(name) => write!(f, "{} ", name.as_str())?,
                CorridorIdentity::Anonymous(id) => write!(f, "_:{id} ")?,
            }
        }
        f.write_str(if chain.severed { "severed" } else { "root" })
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
            // A hit on ⊥ is forwarded untouched, and `wrap` never runs: a governor
            // decorates what will be INVOKED, and a limiter is never invoked. Were
            // the wrapper built, its own `is_limiter()` — defaulted `false` on a
            // type that has never heard of limiters — would turn the hole back
            // into a door, and every governor in the ecosystem would un-limit
            // whatever it fronted. See [`Limit`].
            Resolution::Hit(hit) if hit.endpoint.is_limiter() => Resolution::Hit(hit),
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
/// ## ★ [`Resolved::new`], or a struct literal? Both — the question is whether
/// this site has a claim to make
///
/// **[`Resolved::new`] where it does not.** A site that forwards, wraps, or is
/// genuinely indifferent to the defaults has nothing to say about a field that
/// does not exist yet, and should not be edited when one lands. That is the
/// overwhelmingly common case and it stays the default advice. Better still, an
/// overlay decorating a resolution it did not make should not build a `Resolved`
/// at all: [`Resolution::map_endpoint`] and [`Resolved::with_endpoint`] keep
/// everything the inner resolution reported, including its
/// [`canonical`](Resolved::canonical).
///
/// **The struct literal — field spelled out, reason beside it — where a default
/// IS a considered semantic claim.** Two live ones, both in `ikigai-cli`:
/// `ikigai-module` originates the resolution and rewrites nothing, so
/// `canonical: None` is a statement about the module boundary rather than an
/// absence of thought; `MountedRemote` *does* rewrite (`urn:edge:foo` →
/// `urn:foo`) and must still report `None`, because that rewrite crosses out of
/// this kernel's namespace (see [`canonical`](Resolved::canonical)). At those
/// sites the compile break on the next added field is the **point**: it forces a
/// re-review of a claim that may not survive the type learning to say more.
///
/// ## ★ The cost of the literal, which is bigger than an edit
///
/// [`SpaceEntry`] taught this repo the mechanical half of the lesson at 0.1.7 —
/// the `ikigai-web-demo` manifest still carries the epitaph. The half nobody had
/// counted is that **the break crosses the published-crate boundary.** A struct
/// literal in a *published* consumer means a core bump cannot be adopted anywhere
/// downstream until that consumer has cut a release carrying the conversion. When
/// `canonical` landed in 0.1.64, `ikigai-cli` was simultaneously 100% correct and
/// 100% uncompilable, waiting on an unrelated crate's publish.
///
/// So the literal is a standing tax on the whole ecosystem's release ORDER, not
/// just on the file it appears in. Pay it where a forced re-review is genuinely
/// wanted; take [`Resolved::new`] everywhere else.
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
    ///
    /// # ★ Precondition: a reported canonical is a name in THIS kernel's namespace
    ///
    /// `canonical` does not mean "a name I rewrote to". It means **the same
    /// resource under another name that this kernel resolves** — the kernel adopts
    /// it as the request's identity, so reporting it asserts that anything
    /// resolving that name *here* reaches this very resource.
    ///
    /// A rewrite that crosses out of this kernel therefore reports nothing even
    /// though it rewrote. A mount stripping its local prefix (`urn:edge:foo` →
    /// `urn:foo`) has produced a **wire address**, meaningful in the remote; this
    /// kernel may serve an entirely unrelated local `urn:foo`, and reporting the
    /// stripped name would fuse two different resources into one cache entry and
    /// one golden thread. That is precisely the mistake a mechanical sweep makes —
    /// it sees a rewrite and forwards it — so the rule is stated here, on the
    /// field, and not only where a mount happens to be written. Sharing identity
    /// across an origin would need a concept that carries the origin *alongside*
    /// the name; a bare [`Iri`] cannot express it.
    ///
    /// What "adopted as identity" buys, and therefore what a wrong report costs —
    /// two names, one cache entry:
    ///
    /// ```
    /// use std::sync::Arc;
    /// use futures::executor::block_on;
    /// use ikigai_core::{
    ///     builtins, ArgRef, Capability, EndpointSpace, Exact, Iri, Kernel, Request, Rewrite,
    ///     Verb,
    /// };
    ///
    /// let backing =
    ///     EndpointSpace::new().bind(Exact::new("urn:iki:fn:toUpper"), builtins::to_upper());
    /// // The rewrite reports what it rewrote; this kernel holds no alias table.
    /// let kernel = Kernel::new(Arc::new(Rewrite::new(Arc::new(backing), |iri: &Iri| {
    ///     iri.as_str()
    ///         .strip_prefix("urn:fn:")
    ///         .and_then(|rest| Iri::parse(format!("urn:iki:fn:{rest}")).ok())
    /// })));
    /// let cap = Capability::root();
    /// let up = |name: &str| {
    ///     Request::new(Verb::Source, Iri::parse(name).unwrap())
    ///         .with_arg("in", ArgRef::Inline(b"hi".to_vec()))
    /// };
    ///
    /// let logical = block_on(kernel.issue(up("urn:fn:toUpper"), &cap)).unwrap();
    /// let backing = block_on(kernel.issue(up("urn:iki:fn:toUpper"), &cap)).unwrap();
    /// assert_eq!(logical.bytes, b"HI");
    /// assert_eq!(backing.bytes, b"HI");
    ///
    /// // ONE entry: the logical name was cached under the canonical the rewrite
    /// // reported. Report a name this kernel does not actually resolve to this
    /// // resource and that single shared entry becomes a collision instead.
    /// assert_eq!(kernel.cache_len(), 1);
    /// ```
    pub canonical: Option<Iri>,
    /// The identity of the space whose door matched — the **innermost** space on
    /// the resolution path that claims one ([`Space::id`]) — or, when no space on
    /// the path claims an identity, the name of the injected corridor the hit came
    /// from ([`Scope::with_named`]); `None` when nothing on the path is named.
    ///
    /// Whoever has a name reports it, and an inner report is kept: a leaf sets it
    /// when it has an id, and every combinator with an id fills it only if the
    /// space it delegated to did not (the rule [`with_canonical`](Self::with_canonical)
    /// uses, so a `Rewrite` under a named `Mount` reports the innermost named
    /// answerer). Combinators without a name forward it unchanged, as
    /// [`with_endpoint`](Self::with_endpoint) and [`Resolution::map_endpoint`] keep
    /// it through decoration. The kernel discloses it on every traced event under
    /// [`ANSWERED_NOTE`](crate::ANSWERED_NOTE).
    ///
    /// It is the datum a cache keyed on the corridors actually consulted (the
    /// paper's §5.1 remedy for the whole-chain fingerprint) would key on. That
    /// caching is **not built** — the fingerprint still covers the whole chain —
    /// and this field is where it starts.
    pub answered_by: Option<Iri>,
}

impl Resolved {
    /// A resolution that did not rewrite the target and names no answerer.
    pub fn new(endpoint: Arc<dyn Endpoint>, bindings: Bindings) -> Self {
        Resolved {
            endpoint,
            bindings,
            canonical: None,
            answered_by: None,
        }
    }

    /// Report the space that answered (builder). An already-reported answerer is
    /// *kept*: the innermost named space on the path is the one that answered.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use ikigai_core::{builtins, Bindings, Endpoint, Iri, Resolved};
    ///
    /// let endpoint: Arc<dyn Endpoint> = Arc::new(builtins::to_upper());
    /// let inner = Resolved::new(endpoint, Bindings::default())
    ///     .with_answered_by(Iri::parse("urn:example:space:leaf").unwrap());
    /// // A named combinator enclosing it does not overwrite the leaf's report.
    /// let outer = inner.with_answered_by(Iri::parse("urn:example:space:mount").unwrap());
    /// assert_eq!(outer.answered_by.unwrap().as_str(), "urn:example:space:leaf");
    /// ```
    pub fn with_answered_by(mut self, space: Iri) -> Self {
        self.answered_by.get_or_insert(space);
        self
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
    ///
    /// **⊥ is never substituted.** A resolution onto a limiter
    /// ([`Endpoint::is_limiter`]) keeps its endpoint whatever is offered: a
    /// decorated limiter is still a limiter, because the alternative — a wrapper
    /// whose defaulted `is_limiter()` says `false` — would let any overlay
    /// un-limit a family by wrapping it, which is the one thing a structural bound
    /// must not yield to. The way to make a limited family resolvable again is to
    /// bind it AHEAD of the limiter, not to decorate the hole. See [`Limit`].
    pub fn with_endpoint(mut self, endpoint: Arc<dyn Endpoint>) -> Self {
        if !self.endpoint.is_limiter() {
            self.endpoint = endpoint;
        }
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
    /// "`urn:example:compose` — local / via `beefybox`" instead of an anonymous
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
/// [`Mount`], [`Fallback`], [`Rewrite`] and [`Limit`] combinators.
pub trait Space: Send + Sync {
    /// Resolve a request to an endpoint, or report a miss.
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution;

    /// Enumerate this space's bindings, if it can. `None` means the space does
    /// not support enumeration (e.g. a rewrite or a remote space); `Some(vec![])`
    /// means it is enumerable but empty. The default is `None`.
    ///
    /// What is listed is what is **bound**, not what is reachable: a pattern list
    /// cannot decide whether a template falls inside a limiter's family, so a
    /// later member's pattern inside a limited family is still listed here. Every
    /// face that computes reach (`urn:kernel:catalog`, `urn:kernel:actions`,
    /// selection) probes each pattern through resolution and subtracts a hit on ⊥.
    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        None
    }

    /// The identity this space claims for itself, if any. The default is `None`:
    /// a space is anonymous unless it says otherwise (`.named(iri)` on every core
    /// combinator).
    ///
    /// **A name is a claim**: the same one [`Scope::with_named`] makes for a
    /// corridor and [`Resolved::canonical`] makes for a rewritten name — *any
    /// space named `n` holds the same doors as this one.* The cache partitions on
    /// it, [`Resolved::answered_by`] reports it, and `urn:kernel:topology` names
    /// the node by it. Name two different arrangements alike and one request is
    /// served the other's answers; name one arrangement consistently and every
    /// corridor built from it shares one cache entry and one node.
    fn id(&self) -> Option<Iri> {
        None
    }

    /// This space's structure as a tree, for `urn:kernel:topology`. The default
    /// answers an [opaque](SpaceKind::Opaque) node carrying the space's
    /// [`id`](Self::id): a space that does not say what it encloses is reported
    /// as saying nothing, which is the honest answer — the graph states where
    /// structural knowledge stops rather than implying the space is empty. Every
    /// core combinator overrides it; an overlay that encloses one space should
    /// forward it, as it forwards [`entries`](Self::entries).
    fn topology(&self) -> Topology {
        Topology::opaque(self.id())
    }
}

/// A shared space **is** a space, so anything generic over `S: Space` accepts an
/// already-erased one.
///
/// The composition algebra hands back `Arc<dyn Space>` — [`Mount`], [`Fallback`],
/// [`Rewrite`], [`Alias`](crate::Alias) and `ikigai-throttle`'s `Failover` all
/// compose from and into it. But the interception-overlay family is written
/// `impl<S: Space>` (a governor owns its inner space by value), so without this
/// impl the moment a stack erases you cannot put a governor on top without
/// hand-writing a delegating newtype. For a family whose whole pitch is "stack
/// them in front of anything", that was a wall; it stopped the throttle arc's
/// tests the first time they tried it.
///
/// `?Sized` is what makes it cover `Arc<dyn Space>` and not merely
/// `Arc<Concrete>`. [`Space`] has only `&self` methods, so the delegation is
/// total — there is nothing an `Arc` cannot forward.
///
/// One consequence worth naming so nobody puzzles over it: afterwards both `X`
/// and `Arc<X>` implement `Space`, so `Arc<Arc<dyn Space>>` compiles and quietly
/// adds one pointer hop per resolution. Harmless, and no more than that — it is
/// not a cycle, a double-wrap, or a change in semantics.
impl<S: Space + ?Sized> Space for Arc<S> {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        (**self).resolve(request, scope)
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        (**self).entries()
    }

    fn id(&self) -> Option<Iri> {
        (**self).id()
    }

    fn topology(&self) -> Topology {
        (**self).topology()
    }
}

/// A leaf space: an ordered set of `(grammar, endpoint)` bindings. The first
/// grammar that matches the request's target wins.
#[derive(Default)]
pub struct EndpointSpace {
    bindings: Vec<(Box<dyn Grammar>, Arc<dyn Endpoint>)>,
    id: Option<Iri>,
}

impl EndpointSpace {
    /// An empty leaf space.
    pub fn new() -> Self {
        EndpointSpace {
            bindings: Vec::new(),
            id: None,
        }
    }

    /// Claim an identity for this space (builder): what [`Space::id`] answers,
    /// what a hit through it reports as [`Resolved::answered_by`], what
    /// [`Scope::with`] injects it under, and the IRI `urn:kernel:topology` names
    /// its node by.
    ///
    /// **The name is a claim — same name ⇒ same doors.** It is the same claim
    /// [`Scope::with_named`] makes for a corridor and [`Resolved::canonical`] for a
    /// rewritten name: the cache partitions on it, so two spaces named alike must
    /// hold the same doors, and one space named consistently shares one entry
    /// however often it is rebuilt.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use futures::executor::block_on;
    /// use ikigai_core::{
    ///     Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation,
    ///     Request, Scope, Space, Verb,
    /// };
    ///
    /// let pinned = || {
    ///     Arc::new(
    ///         EndpointSpace::new()
    ///             .bind(
    ///                 Exact::new("urn:time:now"),
    ///                 FnEndpoint::new("pinned", |_| {
    ///                     Ok(Representation::new(ReprType::new("text/plain"), b"18:00Z".to_vec())
    ///                         .cacheable())
    ///                 }),
    ///             )
    ///             .named(Iri::parse("urn:example:ctx:time:2026-09-25T18:00Z").unwrap()),
    ///     )
    /// };
    /// assert_eq!(pinned().id().unwrap().as_str(), "urn:example:ctx:time:2026-09-25T18:00Z");
    ///
    /// // The space says who it is, so `with` injects it under that name: two
    /// // requests, two freshly built spaces, ONE cache entry.
    /// let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    /// let cap = Capability::root();
    /// let now = || Request::new(Verb::Source, Iri::parse("urn:time:now").unwrap());
    /// block_on(kernel.issue_in(now(), &cap, Scope::empty().with(pinned()))).unwrap();
    /// block_on(kernel.issue_in(now(), &cap, Scope::empty().with(pinned()))).unwrap();
    /// assert_eq!(kernel.cache_len(), 1);
    /// ```
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
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
                let resolved = Resolved::new(Arc::clone(endpoint), bindings);
                return Resolution::Hit(match &self.id {
                    Some(id) => resolved.with_answered_by(id.clone()),
                    None => resolved,
                });
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

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    fn topology(&self) -> Topology {
        Topology::new(SpaceKind::EndpointSpace {
            patterns: self
                .bindings
                .iter()
                .map(|(grammar, _)| grammar.pattern())
                .collect(),
        })
        .with_id(self.id.clone())
    }
}

/// Mount a space behind an IRI prefix; only requests whose target starts with
/// the prefix are delegated to the inner space.
pub struct Mount {
    prefix: String,
    inner: Arc<dyn Space>,
    id: Option<Iri>,
}

impl Mount {
    /// Mount `inner` at `prefix`.
    pub fn new(prefix: impl Into<String>, inner: Arc<dyn Space>) -> Self {
        Mount {
            prefix: prefix.into(),
            inner,
            id: None,
        }
    }

    /// Claim an identity for this mount (builder) — a claim, same name ⇒ same
    /// doors; see [`EndpointSpace::named`].
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
    }
}

impl Space for Mount {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        if request.target.as_str().starts_with(&self.prefix) {
            answered(self.inner.resolve(request, scope), &self.id)
        } else {
            Resolution::Miss
        }
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        // The inner space's patterns are already full identifiers.
        self.inner.entries()
    }

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    fn topology(&self) -> Topology {
        Topology::new(SpaceKind::Mount {
            prefix: self.prefix.clone(),
        })
        .with_id(self.id.clone())
        .child(self.inner.topology())
    }
}

/// Fill a hit's [`answered_by`](Resolved::answered_by) with a combinator's own
/// identity when the space it delegated to reported none; a miss passes through.
pub(crate) fn answered(resolution: Resolution, id: &Option<Iri>) -> Resolution {
    match (resolution, id) {
        (Resolution::Hit(hit), Some(id)) => Resolution::Hit(hit.with_answered_by(id.clone())),
        (other, _) => other,
    }
}

/// Try each space in order; the first hit wins.
pub struct Fallback {
    spaces: Vec<Arc<dyn Space>>,
    id: Option<Iri>,
}

impl Fallback {
    /// A fallback over the given spaces, tried in order.
    pub fn new(spaces: Vec<Arc<dyn Space>>) -> Self {
        Fallback { spaces, id: None }
    }

    /// Claim an identity for this fallback (builder) — a claim, same name ⇒ same
    /// doors; see [`EndpointSpace::named`].
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
    }
}

impl Space for Fallback {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        for space in &self.spaces {
            if let Resolution::Hit(resolved) = space.resolve(request, scope) {
                return answered(Resolution::Hit(resolved), &self.id);
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

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    fn topology(&self) -> Topology {
        let mut node = Topology::new(SpaceKind::Fallback).with_id(self.id.clone());
        for space in &self.spaces {
            node = node.child(space.topology());
        }
        node
    }
}

/// The family a [`Limit`] admits: a literal prefix (the shape [`Mount`] takes) or
/// any [`Grammar`] (the shape a binding takes).
enum Family {
    Prefix(String),
    Grammar(Box<dyn Grammar>),
}

/// **A limiter: carve a family of identifiers OUT of a chain, by structure.**
/// The paper's Definition 7 — a door whose endpoint is the distinguished ⊥ — and
/// §9.6's *difference* in the algebra of reachability (import is union, mapper is
/// preimage, limiter is difference).
///
/// For a target in its family a `Limit` answers a **hit** on a kernel-known
/// endpoint ([`Endpoint::is_limiter`]); for anything else it misses. So placed
/// ahead of a member in a [`Fallback`] it stops resolution for the family before
/// that member is consulted — `Fallback([Limit("urn:personal:"), S])` is *S minus
/// `urn:personal:*`* — and the kernel answers exactly what it answers for a name
/// bound nowhere: [`Error::Unresolved`](crate::Error::Unresolved), the same
/// variant, the same text. Not a denial: a denial is a decision that reveals a
/// binding exists (§9.6's remark on what a boundary reveals); a limited name has
/// nowhere to go. Nothing is stored, no capability floor is evaluated, `Meta` on
/// the name is unresolved too (describing a hole would reveal it), and the cache
/// probe answers `false`. The trace alone may know ([`LIMITED_NOTE`](crate::LIMITED_NOTE)).
///
/// # Enumeration subtracts
///
/// A limiter [`entries`](Space::entries) as `Some(vec![])` — enumerable, and
/// binding nothing a caller may reach. A later member's pattern inside the family
/// still appears in the raw concatenation of [`Fallback::entries`] (a list of
/// PATTERNS, which cannot decide membership of a template in a grammar's family),
/// but every `entries → Meta → describe` walk — `urn:kernel:catalog`,
/// `urn:kernel:actions`, [`Kernel::describe`](crate::Kernel::describe),
/// selection, validation — probes each pattern through resolution and **drops** a
/// hit on ⊥, so the manifold never offers a limited name. That is the reachability
/// algebra's subtraction landing where reach is computed.
///
/// # A limiter is not decorated away
///
/// [`Resolution::map_endpoint`] forwards a hit on ⊥ without running the wrapper,
/// and [`Resolved::with_endpoint`] keeps ⊥ whatever it is offered: a governor
/// stacked over a limited family still limits it. The way to make the family
/// reachable again is structural — bind it *ahead* of the limiter.
///
/// # At a position
///
/// A `Limit` is a [`Space`], so it is also a **corridor**: injected with
/// [`Scope::with_named`] it limits the family for that request and its
/// sub-requests and for nothing else — the paper's "limiter at a position", with
/// no new mechanism. Inside a confinement whose space is `Fallback([Limit(..), S])`
/// a limited name is unresolved, and the trace says *limited*, not
/// *scope-unresolved*: the chain **has** a door for it, and the door is ⊥.
///
/// # Structure instead of a construction site
///
/// Before this the only way to keep `urn:personal:*` off a served surface was to
/// build a different root per process (`ikigai-embedded`'s `base_space` /
/// `served_space` / `local_space`, chosen by grant). With a limiter the host
/// serves its ONE space with the family carved out, and the same space object is
/// used everywhere:
///
/// ```
/// use std::sync::Arc;
/// use futures::executor::block_on;
/// use ikigai_core::{
///     Capability, EndpointSpace, Error, Exact, Fallback, FnEndpoint, Iri, Kernel, Limit,
///     ReprType, Representation, Request, Space, Verb,
/// };
///
/// fn text(s: &str) -> Representation {
///     Representation::new(ReprType::new("text/plain"), s.as_bytes().to_vec())
/// }
/// // ONE space: a public door and a personal one.
/// let root: Arc<dyn Space> = Arc::new(
///     EndpointSpace::new()
///         .bind(Exact::new("urn:public:hello"), FnEndpoint::new("hello", |_| Ok(text("hi"))))
///         .bind(Exact::new("urn:personal:calendar"), FnEndpoint::new("cal", |_| Ok(text("…")))),
/// );
/// let cap = Capability::root();
/// let get = |name: &str| Request::new(Verb::Source, Iri::parse(name).unwrap());
///
/// // The construction-site form: a smaller root per process — TWO spaces to keep
/// // in step. (What `ikigai-embedded` does today.)
/// let local = Kernel::new(Arc::clone(&root));
/// assert!(block_on(local.issue(get("urn:personal:calendar"), &cap)).is_ok());
///
/// // The structural form: the SAME root, served behind a limiter.
/// let served = Kernel::new(Arc::new(Fallback::new(vec![
///     Arc::new(Limit::new("urn:personal:")),
///     Arc::clone(&root),
/// ])));
/// assert!(block_on(served.issue(get("urn:public:hello"), &cap)).is_ok());
/// let limited = block_on(served.issue(get("urn:personal:calendar"), &cap)).unwrap_err();
/// // …and the answer is the one an unbound name gets, byte for byte.
/// let unbound = block_on(served.issue(get("urn:nowhere:x"), &cap)).unwrap_err();
/// assert!(matches!(limited, Error::Unresolved(_)));
/// assert_eq!(
///     limited.to_string().replace("urn:personal:calendar", "urn:nowhere:x"),
///     unbound.to_string()
/// );
/// ```
///
/// For the day ikigai has a boot: the paper's §11 notes that limiters, refusals
/// and catch-alls are the ONLY constructs that make an annealing boot
/// non-monotone — adding a limiter can make a name that resolved stop resolving,
/// so a boot that composes spaces incrementally cannot treat a `Limit` as a plain
/// addition.
pub struct Limit {
    family: Family,
    /// The distinguished endpoint, shared by every hit this limiter answers.
    bottom: Arc<dyn Endpoint>,
    id: Option<Iri>,
}

impl Limit {
    /// A limiter over every identifier under `prefix` — the family shape
    /// [`Mount`] takes.
    pub fn new(prefix: impl Into<String>) -> Self {
        Limit {
            family: Family::Prefix(prefix.into()),
            bottom: Arc::new(Bottom),
            id: None,
        }
    }

    /// A limiter over the family a grammar accepts — an [`Exact`](crate::Exact)
    /// name, a [`UriTemplate`](crate::UriTemplate), or any decidable
    /// [`Grammar`]; the captures are discarded.
    pub fn matching(grammar: impl Grammar + 'static) -> Self {
        Limit {
            family: Family::Grammar(Box::new(grammar)),
            bottom: Arc::new(Bottom),
            id: None,
        }
    }

    /// Claim an identity for this limiter (builder) — a claim, same name ⇒ same
    /// family; see [`EndpointSpace::named`]. A hit on ⊥ then reports the limiter
    /// as its [`answered_by`](Resolved::answered_by), so a trace can say WHICH
    /// limiter carved the name out.
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
    }

    /// The family as text: the prefix, or the grammar's pattern.
    fn family(&self) -> String {
        match &self.family {
            Family::Prefix(prefix) => prefix.clone(),
            Family::Grammar(grammar) => grammar.pattern(),
        }
    }

    fn admits(&self, iri: &Iri) -> bool {
        match &self.family {
            Family::Prefix(prefix) => iri.as_str().starts_with(prefix.as_str()),
            Family::Grammar(grammar) => grammar.match_iri(iri).is_some(),
        }
    }
}

impl Space for Limit {
    fn resolve(&self, request: &Request, _scope: &Scope) -> Resolution {
        if self.admits(&request.target) {
            answered(
                Resolution::Hit(Resolved::new(Arc::clone(&self.bottom), Bindings::new())),
                &self.id,
            )
        } else {
            Resolution::Miss
        }
    }

    /// Enumerable, and empty: a limiter binds nothing a caller may reach.
    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        Some(Vec::new())
    }

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    fn topology(&self) -> Topology {
        Topology::new(SpaceKind::Limit {
            family: self.family(),
        })
        .with_id(self.id.clone())
    }
}

/// ⊥ — the distinguished endpoint a [`Limit`] resolves to. The kernel recognizes
/// it by [`Endpoint::is_limiter`] and never invokes it. Should something else
/// invoke it — a harness that matches [`Resolution`] and calls `invoke` on a hit
/// without asking — it answers as the kernel would: the target is unresolved.
struct Bottom;

#[async_trait::async_trait]
impl Endpoint for Bottom {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        Err(Error::Unresolved(inv.request.target.clone()))
    }

    fn name(&self) -> &str {
        "bottom"
    }

    fn is_limiter(&self) -> bool {
        true
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
    id: Option<Iri>,
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
            id: None,
        }
    }

    /// Claim an identity for this rewrite (builder) — a claim, same name ⇒ same
    /// doors; see [`EndpointSpace::named`].
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
    }
}

impl Space for Rewrite {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let resolution = match (self.rule)(&request.target) {
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
        };
        // The innermost named space answered; this one only fills an absence.
        answered(resolution, &self.id)
    }

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    /// The rule is a closure, so the table is opaque: the node says a rewrite
    /// happens here and what it encloses, and nothing about τ.
    fn topology(&self) -> Topology {
        Topology::new(SpaceKind::Rewrite)
            .with_id(self.id.clone())
            .child(self.inner.topology())
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
            .bind(Exact::new("urn:test:to-upper"), builtins::to_upper())
            .bind(
                UriTemplate::parse("urn:demo:echo/{message}").unwrap(),
                builtins::echo(),
            );
        let entries = space.entries().expect("enumerable");
        assert_eq!(
            entries,
            vec![
                SpaceEntry::new("urn:test:to-upper", "toUpper"),
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

    /// The interception-overlay shape: a governor OWNS its inner space and is
    /// generic over it, the way every one in `ikigai-throttle` is written.
    struct Governor<S: Space>(S);

    impl<S: Space> Space for Governor<S> {
        fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
            self.0.resolve(request, scope)
        }

        fn entries(&self) -> Option<Vec<SpaceEntry>> {
            self.0.entries()
        }
    }

    #[test]
    fn a_governor_stacks_on_an_already_erased_space() {
        // ★ The wall this closes: the composition algebra hands back
        // `Arc<dyn Space>`, so without `impl Space for Arc<S>` an overlay written
        // `impl<S: Space>` could not be put on top of anything composed — it would
        // need a hand-written delegating newtype per stack.
        let erased: Arc<dyn Space> =
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:x"), builtins::to_upper()));
        let stacked = Governor(Arc::clone(&erased));

        let hit = stacked.resolve(
            &Request::new(crate::Verb::Source, Iri::parse("urn:x").unwrap()),
            &Scope::empty(),
        );
        assert!(matches!(hit, Resolution::Hit(_)));
        assert_eq!(
            stacked.entries().expect("enumerable")[0].pattern,
            "urn:x",
            "the blanket impl must forward `entries`, not fall back to the default `None`"
        );

        // And re-erasing composes: a governor over an erased space is itself a
        // space, so the next layer up sees no difference.
        let restacked = Governor(Arc::new(stacked) as Arc<dyn Space>);
        assert!(matches!(
            restacked.resolve(
                &Request::new(crate::Verb::Source, Iri::parse("urn:x").unwrap()),
                &Scope::empty()
            ),
            Resolution::Hit(_)
        ));
    }

    // ---- the limiter ----------------------------------------------------------

    fn source(target: &str) -> Request {
        Request::new(crate::Verb::Source, Iri::parse(target).unwrap())
    }

    #[test]
    fn a_limiter_hits_bottom_inside_its_family_and_misses_outside_it() {
        let by_prefix = Limit::new("urn:personal:");
        match by_prefix.resolve(&source("urn:personal:x"), &Scope::empty()) {
            Resolution::Hit(hit) => {
                assert!(hit.endpoint.is_limiter());
                assert!(hit.canonical.is_none(), "a limiter rewrites nothing");
            }
            Resolution::Miss => panic!("a name in the family must hit ⊥, not fall through"),
        }
        assert!(matches!(
            by_prefix.resolve(&source("urn:public:x"), &Scope::empty()),
            Resolution::Miss
        ));

        // The grammar form takes what a binding takes.
        let by_grammar = Limit::matching(UriTemplate::parse("urn:doc:{id}:secret").unwrap());
        assert!(matches!(
            by_grammar.resolve(&source("urn:doc:7:secret"), &Scope::empty()),
            Resolution::Hit(hit) if hit.endpoint.is_limiter()
        ));
        assert!(matches!(
            by_grammar.resolve(&source("urn:doc:7"), &Scope::empty()),
            Resolution::Miss
        ));
    }

    #[test]
    fn a_limiter_is_enumerable_and_binds_nothing() {
        // `Some(vec![])`, not `None`: the limiter's family is fully known — nothing
        // in it is reachable — so it must not make a `Fallback` of limiters and
        // rewrites read as "cannot say".
        assert_eq!(Limit::new("urn:personal:").entries(), Some(Vec::new()));
        let s = Arc::new(EndpointSpace::new().bind(Exact::new("urn:personal:x"), builtins::echo()));
        let fallback = Fallback::new(vec![Arc::new(Limit::new("urn:personal:")), s]);
        // The raw concatenation still lists the pattern — it is a list of what is
        // BOUND; the walks that compute reach subtract it (see `select::probe`).
        assert_eq!(
            fallback.entries().expect("enumerable"),
            vec![SpaceEntry::new("urn:personal:x", "echo")]
        );
        // …and resolution stops on ⊥ before the later member is consulted.
        assert!(matches!(
            fallback.resolve(&source("urn:personal:x"), &Scope::empty()),
            Resolution::Hit(hit) if hit.endpoint.is_limiter()
        ));
    }

    #[test]
    fn a_decorated_limiter_is_still_a_limiter() {
        // A governor wraps every endpoint it fronts with a type of its own; that
        // type's `is_limiter()` is the trait default, `false`. If decoration
        // reached ⊥, stacking a governor over a limited family would silently
        // un-limit it. So `map_endpoint` forwards a hit on ⊥ without running the
        // wrapper, and `with_endpoint` keeps ⊥ whatever it is offered.
        struct Wrapped(Arc<dyn Endpoint>);
        #[async_trait::async_trait]
        impl Endpoint for Wrapped {
            async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
                self.0.invoke(inv).await
            }
        }
        let wrapped = std::sync::atomic::AtomicU32::new(0);
        let limited = Fallback::new(vec![
            Arc::new(Limit::new("urn:personal:")),
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:personal:x"), builtins::echo())),
        ]);
        let wrap = |endpoint: Arc<dyn Endpoint>| -> Arc<dyn Endpoint> {
            wrapped.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Arc::new(Wrapped(endpoint))
        };

        let hit = limited
            .resolve(&source("urn:personal:x"), &Scope::empty())
            .map_endpoint(wrap);
        assert!(matches!(&hit, Resolution::Hit(hit) if hit.endpoint.is_limiter()));
        assert_eq!(
            wrapped.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the wrapper must not run for a hit on ⊥"
        );

        let Resolution::Hit(hit) = hit else {
            unreachable!()
        };
        let kept = hit.with_endpoint(Arc::new(Wrapped(Arc::new(builtins::echo()))));
        assert!(
            kept.endpoint.is_limiter(),
            "`with_endpoint` substituted ⊥ away"
        );

        // The ordinary case is untouched: a real door IS decorated.
        let public = EndpointSpace::new().bind(Exact::new("urn:public:x"), builtins::echo());
        let hit = public
            .resolve(&source("urn:public:x"), &Scope::empty())
            .map_endpoint(wrap);
        assert!(matches!(&hit, Resolution::Hit(hit) if !hit.endpoint.is_limiter()));
        assert_eq!(wrapped.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn bottom_invoked_directly_answers_unresolved() {
        // A harness that matches `Resolution` itself and invokes a hit without
        // asking `is_limiter` — every consumer written before limiters existed —
        // still gets the kernel's answer, not a panic and not a representation.
        let Resolution::Hit(hit) =
            Limit::new("urn:personal:").resolve(&source("urn:personal:x"), &Scope::empty())
        else {
            panic!("in the family")
        };
        let request = source("urn:personal:x");
        let bindings = Bindings::new();
        let cap = crate::Capability::root();
        let inv = Invocation::detached(&request, &bindings, &cap);
        let err = futures::executor::block_on(hit.endpoint.invoke(&inv)).unwrap_err();
        assert!(matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:personal:x"));
    }
}
