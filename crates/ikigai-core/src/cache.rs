//! The representation cache: what the kernel keeps, how long it stays valid, and
//! what it is willing to forget.
//!
//! Three concerns live here, and keeping them apart is the whole design:
//!
//! 1. **Validity** — golden-thread edges ([`Thread`] generations) and the time
//!    deadline on [`Expiry::At`]. This decides whether an entry is *correct* to
//!    serve, and nothing else in this module may override it.
//! 2. **Admission and eviction** — [`CachePolicy`]. This decides whether an entry is
//!    *worth keeping*. A policy can never make the kernel serve something stale: it
//!    is consulted only on the way in and on the way out, never on the way back.
//! 3. **Bounds** — [`CacheBound`]. Before this module the cache was two bare
//!    `HashMap`s with no eviction anywhere: entries left only when a lookup happened
//!    to find them invalid, so a long-lived process driven by ungated cacheable
//!    operations grew without limit. Both maps are bounded now.
//!
//! # The lost-cut race
//!
//! A cache entry pins each of its golden threads to the generation that thread held
//! *when the entry was stored*. Reading those generations after the invocation
//! returns is wrong, and silently so:
//!
//! - t0 — a read begins; the endpoint observes state S.
//! - t1 — a `Sink` cuts thread T, bumping its generation 5 → 6.
//! - t2 — the read returns a representation built from S, which is now stale.
//! - t3 — the store pins T at **6**, and the stale representation is filed as valid
//!   against the very cut that should have invalidated it.
//!
//! The entry then stays "fresh" until some future, unrelated cut. [`ReprCache`]
//! closes this with a **cut sequence**: every cut takes the next number from one
//! monotonic counter and is recorded in a bounded log. A request takes a
//! [`CutSnapshot`] before it invokes anything, and [`ReprCache::store`] declines the
//! entry if any thread it depends on was cut after that snapshot. Declining is
//! always safe — not caching is never wrong — so this never tries to reconcile.
//!
//! Two properties of that choice are worth stating, because both were deliberate:
//!
//! - It is **per-thread, not global**. A global "did anything get cut?" check would
//!   be simpler and far more destructive: a one-second graph build on a busy host
//!   would decline to cache because some unrelated resource was written during it,
//!   turning a correctness fix into the kind of ~2000× read regression that
//!   propagation bugs cause. Only a cut to a thread *this result depends on*
//!   declines the store.
//! - It is **conservative when it cannot tell**. The cut log is bounded
//!   ([`CUT_LOG`]); if a snapshot is older than the oldest cut still retained, cuts
//!   in between may have been forgotten, and the store is declined rather than
//!   guessed at.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::body::Body;
use crate::error::Error;
use crate::repr::{Expiry, Provenance, Representation, Stale, Thread, Time};
use crate::request::RequestId;
use crate::uncached::Declined;

/// How many recent cuts the race check remembers. A store whose snapshot predates
/// the oldest retained cut is declined (see the module docs): the bound costs
/// missed caching under an implausible burst, never correctness.
pub const CUT_LOG: usize = 4096;

/// How many failures the cache holds at once (ledger #2657). A failure is small (an
/// error and its edges), but a misbehaving origin can fail under many names; past
/// this the soonest-expiring is forgotten first, which only costs a recompute.
pub(crate) const CACHED_FAILURES: usize = 1024;

/// A cached representation's key: the content-addressed request, the
/// fingerprint of the authority that computed it, **and** the fingerprint of the
/// resolution chain it was computed in.
///
/// The capability half is not decoration. A cache hit is served *before* the
/// endpoint runs, so an entry keyed only by request id would let one authority read
/// another's cached result, skipping the capability check entirely.
///
/// The scope half is the same argument for the chain: a request resolved inside a
/// confined chain, or with a corridor injected ahead of the root, can resolve one
/// name to a *different endpoint* than the same request in the plain chain. A cached
/// representation may be shared across that boundary only when the corridors
/// consulted are the same on both sides; with the whole chain in the key that holds
/// by construction, over-partitioned rather than unsound. The empty chain's
/// fingerprint is `0`, so [`new`](Self::new) — which does not take one — is the empty
/// chain's key, and every key built before scopes existed is unchanged.
///
/// The kernel additionally checks incoming pipe provenance in the private cache
/// slot before reuse. A changed upstream expiry or thread set is a miss, even for
/// the same public key; a successful store replaces that key's previous variant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct CacheKey {
    /// The content-addressed identity of the request that produced the entry.
    pub request: RequestId,
    /// A stable fingerprint of the capability the request was issued under.
    pub capability: u64,
    /// A stable fingerprint of the resolution chain the request was resolved in
    /// ([`Scope::fingerprint`](crate::Scope::fingerprint)); `0` for the empty chain.
    #[serde(default)]
    pub scope: u64,
}

impl CacheKey {
    /// A key over a request id and a capability fingerprint, in the empty
    /// resolution chain (scope fingerprint `0`).
    pub fn new(request: RequestId, capability: u64) -> Self {
        CacheKey {
            request,
            capability,
            scope: 0,
        }
    }

    /// The same key in the resolution chain `scope` fingerprints (builder).
    pub fn in_scope(mut self, scope: u64) -> Self {
        self.scope = scope;
        self
    }
}

/// A cached representation plus the golden-thread edges that keep it valid: each
/// `(thread, generation)` records the generation that thread held when the entry was
/// stored. The entry is valid only while every thread is still at that generation —
/// cut any of them and it is stale.
///
/// `Serialize`/`Deserialize` are derived because everything inside already is, and
/// keeping the door open for an ejected cache costs nothing here. **Serializing one
/// of these is not the hard part of ejection, and this derive is not permission to
/// import one** — `generation` is a per-process counter and means nothing in another
/// instance. See `docs/design/cache-ejection.md`.
///
/// This is the BUFFER face of an entry, exactly as 0.1.112 published it (ledger #2261).
/// Since streamed bodies (ledger #2237) the cache itself holds a body that can be read
/// again (a buffer or a [`Replay`](crate::Replay)) in a private slot of the same shape;
/// [`ReprCache::get_body`] and [`ReprCache::store_body`] are its body face.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    /// The IRI of the request that produced this entry — kept so `urn:kernel:cache`
    /// can name what is cached (the cache is otherwise keyed by a content hash).
    pub target: String,
    /// The cached representation itself.
    pub representation: Representation,
    /// The golden threads this entry depends on, each pinned to a generation.
    pub edges: Vec<(Thread, u64)>,
    /// What it cost to produce, in milliseconds, when the kernel had a clock to
    /// measure with. `None` on a clock-free kernel — a policy that reads it must
    /// degrade rather than assume.
    pub cost_millis: Option<u64>,
    /// How many times this entry has been served since it was stored.
    pub hits: u64,
    /// The cache tick at which it was stored (insertion order — FIFO reads this).
    pub stored_tick: u64,
    /// The cache tick at which it was last served (recency — LRU reads this).
    pub last_used_tick: u64,
}

impl CacheEntry {
    /// The entry's size in bytes: the representation's payload. Metadata overhead is
    /// deliberately not counted — the bound is about what the cache is *holding*,
    /// and a policy comparing entries wants the number it can also reason about.
    pub fn bytes(&self) -> usize {
        self.representation.bytes.len()
    }
}

/// What the cache actually holds for one key: [`CacheEntry`]'s fields over a body that
/// can be read again (ledger #2237). Private, so the public entry keeps its 0.1.112
/// shape (ledger #2261).
#[derive(Debug)]
struct Slot {
    // The incoming provenance is a logical part of the kernel's cache key. Keep
    // one variant per public CacheKey, replacing it on a mismatched computation:
    // this preserves the public key's shape and keeps variants within its bound.
    incoming: Option<Provenance>,
    target: String,
    representation: Representation<Body>,
    edges: Vec<(Thread, u64)>,
    cost_millis: Option<u64>,
    hits: u64,
    stored_tick: u64,
    last_used_tick: u64,
}

/// One failure the cache holds until a stated time (ledger #2657).
#[derive(Debug)]
struct FailedSlot {
    incoming: Option<Provenance>,
    target: String,
    error: Error,
    until: Time,
    threads: BTreeSet<Thread>,
    edges: Vec<(Thread, u64)>,
}

/// A failure served from the cache (ledger #2657): the error as the endpoint answered
/// it, when it stops being served, and the threads a cut drops it by — what a caller
/// that catches it hangs its own answer from.
#[derive(Clone, Debug)]
pub(crate) struct CachedFailure {
    pub(crate) error: Error,
    pub(crate) until: Time,
    pub(crate) threads: BTreeSet<Thread>,
}

/// One cached failure for the readouts (`urn:kernel:cache`, `urn:kernel:uncached`):
/// the IRI that failed, the error it answered, and the time it is served until.
#[derive(Clone, Debug)]
pub(crate) struct FailureRow {
    pub(crate) target: String,
    pub(crate) error: Error,
    pub(crate) until: Time,
    pub(crate) state: EntryState,
}

/// What the kernel's resolution path answers with (ledger #2237): a body, or, for a
/// buffer caller's cache hit, the buffer already copied out of the entry. Internal: the
/// public faces are `Representation<Body>` and `Representation<Vec<u8>>`.
pub(crate) enum Answer {
    /// The body as the endpoint (or the cache) holds it.
    Body(Representation<Body>),
    /// A streamed body (a `Replay` or a `Once`) with the byte pool of the run it was
    /// answered in (ledger #2238): what a buffer caller collects it under.
    Streamed(Representation<Body>, crate::MemoryBudget),
    /// A buffer copied out of a cache entry for a caller that wants bytes.
    Buffer(Representation),
}

impl Answer {
    /// The body face: a copied buffer becomes [`Body::Bytes`] without another copy.
    pub(crate) fn into_body(self) -> Representation<Body> {
        match self {
            Answer::Body(representation) | Answer::Streamed(representation, _) => representation,
            Answer::Buffer(representation) => representation.into_body(),
        }
    }

    /// This answer with the run's byte pool attached when its body is streamed: `pool`
    /// is asked only then, so a buffer answer (every cache hit on a buffer) admits
    /// nothing and allocates nothing.
    pub(crate) fn budgeted(self, pool: impl FnOnce() -> Option<crate::MemoryBudget>) -> Answer {
        match self {
            Answer::Body(representation) if representation.bytes.as_bytes().is_none() => {
                match pool() {
                    Some(pool) => Answer::Streamed(representation, pool),
                    None => Answer::Body(representation),
                }
            }
            other => other,
        }
    }
}

/// What the kernel's lookup found (ledger #2534): an answer to serve, a stale answer to
/// offer the endpoint for revalidation, or nothing.
enum Lookup {
    /// A live entry, answered in the face asked for.
    Hit(Answer),
    /// An entry expired by time only, carrying an origin validator: kept, and offered.
    /// Shared: the kernel's resolution path and its `Revalidate` wrapper each hold one
    /// pointer to it.
    Offer(Arc<Offer>),
    /// Nothing to serve or offer.
    Miss,
}

/// A stale answer offered for revalidation: what the endpoint is told
/// ([`Invocation::stale`](crate::Invocation::stale)), and a second handle on the stored
/// body the kernel serves if the endpoint answers "still valid".
pub(crate) struct Offer {
    /// The stored validator and type, as the invocation hands them to the endpoint.
    pub(crate) stale: Arc<Stale>,
    /// The stored representation: its body, threads and validator. Behind a lock only
    /// because a [`Body`] is not `Sync` as a type; the one held here can always be read
    /// again ([`Offer::stored`]).
    stored: Mutex<Representation<Body>>,
    /// Which entry it was ([`ReprCache::discard`] forgets only that one).
    stored_tick: u64,
}

impl Offer {
    /// A second handle on the stored representation: what the kernel serves when the
    /// endpoint answers "still valid".
    pub(crate) fn stored(&self) -> Option<Representation<Body>> {
        self.stored.lock().expect("offer lock").share()
    }
}

/// [`ReprCache::serve_or_offer`]'s three outcomes.
enum Served<T> {
    Hit(T),
    Offer(Arc<Offer>),
    Miss,
}

/// What a body counts against the byte bound: its length, or `usize::MAX` when it does
/// not know it (see [`Slot::bytes`]).
fn body_size(body: &Body) -> usize {
    body.len_hint()
        .and_then(|len| usize::try_from(len).ok())
        .unwrap_or(usize::MAX)
}

impl Slot {
    /// [`CacheEntry::bytes`] over a body. A [`Replay`](crate::Replay) counts its length
    /// hint, which is conservative for one backed by a file (it holds no memory) until a
    /// spill budget exists; one that does not know its length counts as `usize::MAX`, so
    /// no bounded policy admits it.
    fn bytes(&self) -> usize {
        body_size(&self.representation.bytes)
    }

    fn facts(&self, request: RequestId) -> EntryFacts<'_> {
        EntryFacts {
            request,
            target: &self.target,
            media_type: &self.representation.repr_type.media_type,
            bytes: self.bytes(),
            cost_millis: self.cost_millis,
            hits: self.hits,
            threads: self.edges.len(),
            stored_tick: self.stored_tick,
            last_used_tick: self.last_used_tick,
        }
    }
}

/// What a [`CachePolicy`] may see about one entry — resident or candidate.
///
/// This is the smallest set that supports FIFO ([`Fifo`], on `stored_tick`), LRU
/// ([`Lru`], on `last_used_tick`) and a cost-versus-size heuristic ([`CostAware`], on
/// `cost_millis` and `bytes`), which is the bar the design set. It is a struct rather
/// than a widening argument list so adding a fact later is not a signature change for
/// every implementor.
///
/// Deliberately absent: the representation's bytes and the capability fingerprint.
/// A policy decides what is worth keeping; it has no business reading the payload,
/// and nothing it returns may depend on *whose* entry it is.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EntryFacts<'a> {
    /// The content-addressed request identity, including verb, target and arguments.
    pub request: RequestId,
    /// The IRI the entry was resolved from.
    pub target: &'a str,
    /// The representation's media type.
    pub media_type: &'a str,
    /// The entry's payload size in bytes.
    pub bytes: usize,
    /// What it cost to produce, in milliseconds — `None` on a clock-free kernel.
    pub cost_millis: Option<u64>,
    /// How many times it has been served since being stored (0 for a candidate).
    pub hits: u64,
    /// How many golden threads it depends on.
    pub threads: usize,
    /// The cache tick at which it was stored: a monotonic insertion order.
    pub stored_tick: u64,
    /// The cache tick at which it was last served, or stored if never served.
    pub last_used_tick: u64,
}

/// One resident entry as `urn:kernel:cache` reports it — a human face
/// (ledger #59), not a wire format; `#[non_exhaustive]` so a later fact is not a
/// break for a reader.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CacheRow {
    /// The IRI the entry was resolved from.
    pub target: String,
    /// The representation's media type.
    pub media_type: String,
    /// The entry's payload size in bytes.
    pub bytes: usize,
    /// How many golden threads it depends on.
    pub threads: usize,
    /// The fingerprint of the resolution chain it was computed in
    /// ([`Scope::fingerprint`](crate::Scope::fingerprint)); `0` for the empty chain.
    pub scope: u64,
    /// Whether the entry would be served right now — the same predicate serving and
    /// [`ReprCache::probe`] use (ledger #612).
    pub state: EntryState,
}

/// Whether a resident entry would be served, and if not, why not.
///
/// A cut is LAZY: it bumps a generation and leaves every entry pinned to the old
/// one resident until a read finds it stale and evicts it. So "what the cache holds"
/// and "what the cache would serve" differ right after every edit, and a readout that
/// listed rows without saying which was which read like the edit had invalidated
/// nothing (ledger #612). Stale rows are still worth listing — they occupy the bound
/// until something reads them — so the readout marks them rather than hiding them.
///
/// ```
/// use ikigai_core::EntryState;
///
/// assert_eq!(EntryState::Live.word(), "live");
/// assert_eq!(EntryState::Cut.word(), "cut");
/// assert_eq!(EntryState::Expired.word(), "expired");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EntryState {
    /// Every golden thread still stands at its pinned generation and any deadline is
    /// still ahead: a read is served from this entry.
    Live,
    /// A thread it depends on was cut since it was stored: the next read evicts it
    /// and recomputes.
    Cut,
    /// Its threads stand but its deadline has passed (or the kernel has no clock to
    /// say it has not): the next read evicts it and recomputes, unless it carries an
    /// origin validator, in which case the next read offers it to its endpoint for
    /// revalidation (ledger #2534) and the entry stays until that read refreshes or
    /// replaces it.
    Expired,
}

impl EntryState {
    /// The word `urn:kernel:cache` prints for the state: `live`, `cut` or `expired`.
    pub fn word(self) -> &'static str {
        match self {
            EntryState::Live => "live",
            EntryState::Cut => "cut",
            EntryState::Expired => "expired",
        }
    }

    /// Whether a read would be served from the entry.
    pub fn is_live(self) -> bool {
        self == EntryState::Live
    }
}

/// The ceiling a [`CachePolicy`] enforces: a count and a byte budget, both hard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheBound {
    /// The most entries the cache may hold.
    pub max_entries: usize,
    /// The most bytes of cached representations it may hold in total.
    pub max_bytes: usize,
}

impl CacheBound {
    /// A bound of `max_entries` entries and `max_bytes` bytes.
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        CacheBound {
            max_entries,
            max_bytes,
        }
    }
}

impl Default for CacheBound {
    /// The default bound: **4096 entries and 64 MiB**.
    ///
    /// It is a real number rather than "unbounded, configure it yourself" because
    /// the process that most needs a bound is the edge — the one facing strangers —
    /// and it is exactly the process nobody remembers to configure. The two halves
    /// do different jobs: the byte budget is the memory ceiling, and the entry count
    /// guards against a flood of tiny entries whose per-entry overhead (key, target
    /// IRI, thread names) the byte budget does not see.
    ///
    /// 64 MiB is chosen to sit *above* the working set of the live services
    /// (derived graphs measured in single-digit MiB) and well below anything that
    /// threatens a small VM or a browser tab. A host that knows better should say
    /// so: `Kernel::with_cache_policy(Arc::new(Lru::with_bound(CacheBound::new(256,
    /// 8 << 20))))`.
    fn default() -> Self {
        CacheBound::new(4096, 64 << 20)
    }
}

/// Admission and eviction — never validity.
///
/// The kernel consults a policy in exactly two places: when it is about to store an
/// entry ([`admit`](CachePolicy::admit)), and when the cache is over its
/// [`capacity`](CachePolicy::capacity) and something must go
/// ([`victim`](CachePolicy::victim)). It is never consulted when serving, so no
/// policy — including a hostile one — can make the kernel hand back a representation
/// whose golden thread was cut or whose deadline has passed.
///
/// Implement it to trade differently: keep what was expensive, evict what is large,
/// bias toward what an operator knows is hot. [`Lru`] is the default, [`Fifo`] and
/// [`CostAware`] ship beside it.
pub trait CachePolicy: Send + Sync {
    /// The ceiling this policy enforces. Read on every store, so keep it cheap.
    fn capacity(&self) -> CacheBound {
        CacheBound::default()
    }

    /// Whether to store a candidate at all. The default refuses a single entry
    /// larger than the whole byte budget — admitting it would evict everything else
    /// and still leave the cache over its bound.
    fn admit(&self, candidate: &EntryFacts<'_>) -> bool {
        candidate.bytes <= self.capacity().max_bytes
    }

    /// Pick the resident entry to evict, as an index into `resident`. `None` leaves
    /// the cache over its bound, which the kernel accepts as the policy's decision —
    /// so a policy that wants an unbounded cache says so by returning `None` here,
    /// visibly, rather than by omission.
    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize>;

    /// Whether an admitted candidate is worth what it would displace (ledger #2687).
    /// Asked only when storing it would put the cache over its bound (or, for a
    /// [`partitioned`](Self::partitioned) policy, on every store of a new key), with
    /// the entries resident before it; `false` declines the store exactly as
    /// [`admit`](Self::admit) does, so the residents stay and nothing is evicted.
    ///
    /// The default is `true`: a candidate always displaces a victim, which is what every
    /// policy did before this seam. [`ScanResistant`] answers from how often each name
    /// has been wanted, which is what keeps a one-pass scan out.
    fn contest(&self, candidate: &EntryFacts<'_>, resident: &[EntryFacts<'_>]) -> bool {
        let _ = (candidate, resident);
        true
    }

    /// Whether this policy keeps bounds beyond [`capacity`](Self::capacity) — a budget
    /// per partition of the cache, as [`Partitioned`] does (ledger #2687). Only then is
    /// [`over_partition`](Self::over_partition) asked, and [`contest`](Self::contest)
    /// asked on every store: both read every resident entry, which a policy without
    /// partitions should not pay for.
    fn partitioned(&self) -> bool {
        false
    }

    /// Whether the residents break a bound this policy keeps beyond
    /// [`capacity`](Self::capacity), when the cache as a whole is within it: the kernel
    /// keeps asking for a [`victim`](Self::victim) until it says `false`. Asked only of
    /// a [`partitioned`](Self::partitioned) policy.
    fn over_partition(&self, resident: &[EntryFacts<'_>]) -> bool {
        let _ = resident;
        false
    }

    /// What `urn:kernel:cache` names this policy as (ledger #2687): one line, the policy
    /// and the parameters it was built with. `custom` for a host's own policy that does
    /// not say.
    fn describe(&self) -> String {
        "custom".to_string()
    }
}

/// Evict the least recently served entry. **The default policy.**
///
/// Recency is the right default for a resolution cache because resolution is bursty
/// and composite: a page resolves the same sub-resources repeatedly within a request
/// and then moves on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lru(CacheBound);

impl Lru {
    /// An LRU policy with a non-default bound.
    pub fn with_bound(bound: CacheBound) -> Self {
        Lru(bound)
    }
}

impl CachePolicy for Lru {
    fn capacity(&self) -> CacheBound {
        self.0
    }

    fn describe(&self) -> String {
        describe_bound("lru", self.0)
    }

    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize> {
        argmin(resident, |facts| facts.last_used_tick)
    }
}

/// Evict the oldest entry, however recently it was served.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fifo(CacheBound);

impl Fifo {
    /// A FIFO policy with a non-default bound.
    pub fn with_bound(bound: CacheBound) -> Self {
        Fifo(bound)
    }
}

impl CachePolicy for Fifo {
    fn capacity(&self) -> CacheBound {
        self.0
    }

    fn describe(&self) -> String {
        describe_bound("fifo", self.0)
    }

    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize> {
        argmin(resident, |facts| facts.stored_tick)
    }
}

/// Evict the entry with the least value per byte: what it cost to recompute, times
/// how often it has been wanted, divided by what it occupies.
///
/// ⚠ Cost is measured with the kernel's [`Clock`](crate::Clock). A kernel with no
/// clock measures nothing, and this policy degrades to [`Lru`] rather than treating
/// every entry as free — an unmeasured cost is not a zero cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct CostAware(CacheBound);

impl CostAware {
    /// A cost-versus-size policy with a non-default bound.
    pub fn with_bound(bound: CacheBound) -> Self {
        CostAware(bound)
    }
}

impl CachePolicy for CostAware {
    fn capacity(&self) -> CacheBound {
        self.0
    }

    fn describe(&self) -> String {
        describe_bound("cost-aware", self.0)
    }

    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize> {
        // No entry carries a measured cost ⇒ nothing to weigh; fall back to recency.
        if resident.iter().all(|facts| facts.cost_millis.is_none()) {
            return argmin(resident, |facts| facts.last_used_tick);
        }
        // Integer arithmetic, scaled: a millisecond of compute per KiB held, times
        // one plus the hit count. Ties (including every unmeasured entry, which
        // scores zero) fall back to recency via the tick in the tuple.
        argmin(resident, |facts| {
            let cost = facts.cost_millis.unwrap_or(0);
            let density = cost
                .saturating_mul(1024)
                .saturating_mul(facts.hits.saturating_add(1))
                / (facts.bytes.max(1) as u64);
            (density, facts.last_used_tick)
        })
    }
}

/// Evict what has not earned its place by being wanted again, so that a one-pass scan
/// cannot flush the set that is used over and over (ledger #2687).
///
/// Plain [`Lru`] loses everything to a scan: a river read walks every feed once, in
/// order, and each read evicts the entry the next read will want first. The feed scale
/// round (ikigai-devtools `claude/scale/register.md`, entry 11) measured the default
/// cache going from about 100% hits to none between 300 and 350 mapped feeds. This
/// policy is segmented LRU behind a frequency filter, in the spirit of SLRU and
/// TinyLFU:
///
/// - **Two segments, read off the entries.** An entry that has never been served since
///   it was stored ([`EntryFacts::hits`] `== 0`) is *probationary*; one that has is
///   *protected*. The victim is the least recently served probationary entry, so a scan
///   churns probation and leaves the protected set alone. Protected entries may hold at
///   most `protected_percent` of either half of the bound; past that the protected entry
///   served least often for the time it has been resident goes (a rate, which fades a
///   name that was hot once without a count to age), which is how a working set that
///   has moved on gives way.
/// - **A frequency filter at the door.** The policy counts, in a small sketch keyed by
///   the full request identity, how often each request has been computed, halving
///   every count from time to time so the past fades. When a new entry would displace
///   a probationary one, it is stored only if its request has been wanted more often
///   than the victim's ([`CachePolicy::contest`]); otherwise it is declined and the
///   residents stay. A request asked for once does not get in over a full cache;
///   asked for again, it does. Under
///   a scan wider than the cache that keeps a stable part of the scan resident rather
///   than none of it, which is the difference between LRU's 0% and a hit rate of about
///   the share of the scan the cache can hold.
///
/// The sketch is approximate (counts can collide upward, never downward) and bounded:
/// about four bytes per entry of [`CacheBound::max_entries`], capped at 4 MiB. It is
/// keyed by request identity without capability or scope fingerprints.
#[derive(Debug)]
pub struct ScanResistant {
    bound: CacheBound,
    protected_percent: u8,
    sketch: Mutex<Sketch>,
}

impl Default for ScanResistant {
    fn default() -> Self {
        ScanResistant::with_bound(CacheBound::default())
    }
}

impl ScanResistant {
    /// The share of the bound protected entries may hold when none is given: 80%.
    pub const DEFAULT_PROTECTED_PERCENT: u8 = 80;

    /// A scan-resistant policy over `bound`, protecting up to 80% of it.
    pub fn with_bound(bound: CacheBound) -> Self {
        ScanResistant {
            bound,
            protected_percent: Self::DEFAULT_PROTECTED_PERCENT,
            sketch: Mutex::new(Sketch::new(bound.max_entries)),
        }
    }

    /// The same policy with protected entries allowed `percent` of the bound (builder).
    /// Clamped to `1..=99`: a protected segment of nothing is LRU with a filter, and one
    /// of everything never lets a working set move on.
    pub fn protecting(mut self, percent: u8) -> Self {
        self.protected_percent = percent.clamp(1, 99);
        self
    }

    /// The share of the bound protected entries may hold, in percent.
    pub fn protected_percent(&self) -> u8 {
        self.protected_percent
    }

    /// How often the sketch says `facts`' request has been wanted: the times it was computed,
    /// plus the times it was served while resident.
    fn frequency(&self, sketch: &Sketch, facts: &EntryFacts<'_>) -> u64 {
        u64::from(sketch.estimate(facts.request)).saturating_add(facts.hits)
    }

    /// Whether the protected entries hold more than their share of the bound.
    fn protected_over(&self, resident: &[EntryFacts<'_>]) -> bool {
        let (count, bytes) = resident
            .iter()
            .filter(|facts| facts.hits > 0)
            .fold((0usize, 0usize), |(count, bytes), facts| {
                (count + 1, bytes.saturating_add(facts.bytes))
            });
        let share = |of: usize| {
            usize::try_from(of as u128 * u128::from(self.protected_percent) / 100)
                .unwrap_or(usize::MAX)
        };
        count > share(self.bound.max_entries) || bytes > share(self.bound.max_bytes)
    }
}

impl CachePolicy for ScanResistant {
    fn capacity(&self) -> CacheBound {
        self.bound
    }

    fn admit(&self, candidate: &EntryFacts<'_>) -> bool {
        // Every candidate is a request wanted once more: count it, admitted or not, so a
        // request declined today is let in when it is asked for again.
        self.sketch
            .lock()
            .expect("sketch lock")
            .increment(candidate.request);
        candidate.bytes <= self.bound.max_bytes
    }

    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize> {
        let protected = |facts: &EntryFacts<'_>| facts.hits > 0;
        let lru_of = |keep: &dyn Fn(&EntryFacts<'_>) -> bool| {
            resident
                .iter()
                .enumerate()
                .filter(|(_, facts)| keep(facts))
                .min_by_key(|(_, facts)| facts.last_used_tick)
                .map(|(index, _)| index)
        };
        if let Some(index) = lru_of(&|facts| !protected(facts)) {
            if !self.protected_over(resident) {
                return Some(index);
            }
        }
        // A protected entry gives way by how often it has been served for how long it
        // has been resident — a rate, so a name that was hot once fades as the cache
        // moves on without any count to age — and the least recently served of equals.
        let now = resident
            .iter()
            .map(|facts| facts.last_used_tick)
            .max()
            .unwrap_or(0);
        let rate = |facts: &EntryFacts<'_>| HitRate {
            hits: facts.hits,
            ticks: now.saturating_sub(facts.stored_tick).saturating_add(1),
        };
        resident
            .iter()
            .enumerate()
            .filter(|(_, facts)| protected(facts))
            .min_by(|(_, a), (_, b)| {
                rate(a)
                    .cmp(&rate(b))
                    .then(a.last_used_tick.cmp(&b.last_used_tick))
            })
            .map(|(index, _)| index)
    }

    fn contest(&self, candidate: &EntryFacts<'_>, resident: &[EntryFacts<'_>]) -> bool {
        let Some(victim) = self.victim(resident).and_then(|index| resident.get(index)) else {
            return true;
        };
        // A protected victim means the protected segment is over its share (or nothing
        // is probationary): it is giving way to whatever comes next, unconditionally.
        if victim.hits > 0 {
            return true;
        }
        let sketch = self.sketch.lock().expect("sketch lock");
        self.frequency(&sketch, candidate) > self.frequency(&sketch, victim)
    }

    fn describe(&self) -> String {
        format!(
            "scan-resistant ({} entries, {} bytes, protected {}%)",
            self.bound.max_entries, self.bound.max_bytes, self.protected_percent
        )
    }
}

/// Hits per tick resident, compared without division.
#[derive(Clone, Copy, PartialEq, Eq)]
struct HitRate {
    hits: u64,
    ticks: u64,
}

impl Ord for HitRate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (u128::from(self.hits) * u128::from(other.ticks))
            .cmp(&(u128::from(other.hits) * u128::from(self.ticks)))
    }
}

impl PartialOrd for HitRate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A count-min sketch of how often each request has been wanted, with four-bit counters
/// that halve every `sample` increments, as TinyLFU ages its sketch.
#[derive(Debug)]
struct Sketch {
    /// One counter per byte, saturating at [`Sketch::MAX`].
    counters: Vec<u8>,
    /// `counters.len() - 1`; the length is a power of two.
    mask: usize,
    /// Increments since the last halving.
    additions: usize,
    /// How many increments trigger a halving: ten per entry the cache can hold.
    sample: usize,
}

impl Sketch {
    const MAX: u8 = 15;
    const ROWS: u64 = 4;

    fn new(max_entries: usize) -> Self {
        let width = max_entries
            .saturating_mul(4)
            .clamp(64, 4 << 20)
            .next_power_of_two();
        Sketch {
            counters: vec![0; width],
            mask: width - 1,
            additions: 0,
            sample: max_entries.saturating_mul(10).max(64),
        }
    }

    /// The counter indices for a request, one per row, using its existing content hash.
    fn cells(&self, request: RequestId) -> [usize; Self::ROWS as usize] {
        let content = request.content_id();
        let bytes = content.as_bytes();
        let mut cells = [0; Self::ROWS as usize];
        for (row, cell) in cells.iter_mut().enumerate() {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[row * 8..row * 8 + 8]);
            *cell = (u64::from_le_bytes(word) as usize) & self.mask;
        }
        cells
    }

    fn estimate(&self, request: RequestId) -> u8 {
        self.cells(request)
            .iter()
            .map(|&cell| self.counters[cell])
            .min()
            .unwrap_or(0)
    }

    fn increment(&mut self, request: RequestId) {
        for cell in self.cells(request) {
            let counter = &mut self.counters[cell];
            *counter = counter.saturating_add(1).min(Self::MAX);
        }
        self.additions += 1;
        if self.additions >= self.sample {
            for counter in &mut self.counters {
                *counter /= 2;
            }
            self.additions = 0;
        }
    }
}

/// Separate budgets per name prefix (ledger #2687): a river under `urn:feed:` cannot
/// evict the REPL's working set or another tenant's views, because each partition is
/// held to its own bound and evicts only its own entries.
///
/// Each partition is a prefix of the entry's IRI ([`EntryFacts::target`]) and a policy
/// of its own, whose [`capacity`](CachePolicy::capacity) is the partition's budget and
/// whose victim is chosen among the partition's entries alone; an entry belongs to the
/// longest prefix it starts with, and to the *rest* when it starts with none. The
/// cache's bound is the sum of every partition's and the rest's. The budgets are hard:
/// a partition over its bound evicts even when the cache as a whole has room, so what
/// one partition holds never depends on how full another is.
///
/// ⚠ A partition's own policy is asked for [`admit`](CachePolicy::admit),
/// [`victim`](CachePolicy::victim) and [`contest`](CachePolicy::contest) over the
/// partition's entries; a partitioned policy nested inside a partition is not asked
/// for its own partitions.
pub struct Partitioned {
    /// `(prefix, policy)`, in the order given.
    partitions: Vec<(String, Arc<dyn CachePolicy>)>,
    /// The policy and budget of every entry no prefix claims.
    rest: Arc<dyn CachePolicy>,
}

impl std::fmt::Debug for Partitioned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Partitioned")
            .field("describe", &self.describe())
            .finish()
    }
}

impl Partitioned {
    /// A partitioned policy with no partitions yet: every entry is the rest's, under
    /// `rest` and its bound.
    pub fn new(rest: Arc<dyn CachePolicy>) -> Self {
        Partitioned {
            partitions: Vec::new(),
            rest,
        }
    }

    /// The same policy with entries whose IRI starts with `prefix` held to `policy` and
    /// its bound (builder). A prefix given twice keeps the first.
    pub fn partition(mut self, prefix: impl Into<String>, policy: Arc<dyn CachePolicy>) -> Self {
        let prefix = prefix.into();
        if !self.partitions.iter().any(|(given, _)| *given == prefix) {
            self.partitions.push((prefix, policy));
        }
        self
    }

    /// Which partition `target` belongs to: an index into `partitions`, or
    /// `partitions.len()` for the rest.
    fn partition_of(&self, target: &str) -> usize {
        self.partitions
            .iter()
            .enumerate()
            .filter(|(_, (prefix, _))| target.starts_with(prefix.as_str()))
            .max_by_key(|(_, (prefix, _))| prefix.len())
            .map_or(self.partitions.len(), |(index, _)| index)
    }

    fn policy(&self, partition: usize) -> &Arc<dyn CachePolicy> {
        self.partitions
            .get(partition)
            .map_or(&self.rest, |(_, policy)| policy)
    }

    /// The residents of one partition, with their indices into `resident`.
    fn members<'a>(
        &self,
        partition: usize,
        resident: &[EntryFacts<'a>],
    ) -> (Vec<usize>, Vec<EntryFacts<'a>>) {
        resident
            .iter()
            .enumerate()
            .filter(|(_, facts)| self.partition_of(facts.target) == partition)
            .map(|(index, facts)| (index, *facts))
            .unzip()
    }

    /// How far a partition's residents are over its bound, in bytes and entries, or
    /// `None` when they are within it.
    fn overflow(&self, partition: usize, members: &[EntryFacts<'_>]) -> Option<(usize, usize)> {
        let bound = self.policy(partition).capacity();
        let bytes = members
            .iter()
            .fold(0usize, |total, facts| total.saturating_add(facts.bytes));
        (members.len() > bound.max_entries || bytes > bound.max_bytes).then(|| {
            (
                bytes.saturating_sub(bound.max_bytes),
                members.len().saturating_sub(bound.max_entries),
            )
        })
    }
}

impl CachePolicy for Partitioned {
    fn capacity(&self) -> CacheBound {
        self.partitions
            .iter()
            .map(|(_, policy)| policy.capacity())
            .fold(self.rest.capacity(), |total, bound| {
                CacheBound::new(
                    total.max_entries.saturating_add(bound.max_entries),
                    total.max_bytes.saturating_add(bound.max_bytes),
                )
            })
    }

    fn admit(&self, candidate: &EntryFacts<'_>) -> bool {
        self.policy(self.partition_of(candidate.target))
            .admit(candidate)
    }

    fn victim(&self, resident: &[EntryFacts<'_>]) -> Option<usize> {
        // The partition furthest over its own bound gives up an entry, chosen by its own
        // policy among its own entries. The cache's bound is the sum of the partitions',
        // so a cache over it always has one.
        let (indices, members, partition) = (0..=self.partitions.len())
            .filter_map(|partition| {
                let (indices, members) = self.members(partition, resident);
                let overflow = self.overflow(partition, &members)?;
                Some((indices, members, (overflow, partition)))
            })
            .max_by_key(|(_, _, (overflow, partition))| (*overflow, std::cmp::Reverse(*partition)))
            .map(|(indices, members, (_, partition))| (indices, members, partition))?;
        let chosen = self.policy(partition).victim(&members)?;
        indices.get(chosen).copied()
    }

    fn contest(&self, candidate: &EntryFacts<'_>, resident: &[EntryFacts<'_>]) -> bool {
        let partition = self.partition_of(candidate.target);
        let policy = self.policy(partition);
        let (_, members) = self.members(partition, resident);
        let bound = policy.capacity();
        let bytes = members.iter().fold(candidate.bytes, |total, facts| {
            total.saturating_add(facts.bytes)
        });
        if members.len() < bound.max_entries && bytes <= bound.max_bytes {
            return true;
        }
        policy.contest(candidate, &members)
    }

    fn partitioned(&self) -> bool {
        true
    }

    fn over_partition(&self, resident: &[EntryFacts<'_>]) -> bool {
        (0..=self.partitions.len()).any(|partition| {
            let (_, members) = self.members(partition, resident);
            self.overflow(partition, &members).is_some()
        })
    }

    fn describe(&self) -> String {
        let mut out = String::from("partitioned (");
        for (prefix, policy) in &self.partitions {
            out.push_str(&format!("{prefix}* {}; ", policy.describe()));
        }
        out.push_str(&format!("rest {})", self.rest.describe()));
        out
    }
}

/// `word` and its bound, as [`CachePolicy::describe`] says it: `lru (4096 entries, 67108864 bytes)`.
fn describe_bound(word: &str, bound: CacheBound) -> String {
    format!(
        "{word} ({} entries, {} bytes)",
        bound.max_entries, bound.max_bytes
    )
}

/// The index of the smallest score, or `None` for an empty slice. Ties take the
/// first, which is stable given the unique ticks every score carries.
fn argmin<T: Ord>(
    resident: &[EntryFacts<'_>],
    score: impl Fn(&EntryFacts<'_>) -> T,
) -> Option<usize> {
    resident
        .iter()
        .enumerate()
        .min_by_key(|(_, facts)| score(facts))
        .map(|(index, _)| index)
}

/// A point in the kernel's cut history, taken **before** a request invokes anything.
/// [`ReprCache::store`] compares it against the cuts that have happened since; see
/// the module docs for why the comparison has to run in that direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CutSnapshot(u64);

/// The kernel's representation cache: valid entries, thread generations, and the
/// bounded cut log that makes the race check possible.
pub struct ReprCache {
    state: Mutex<CacheState>,
    policy: Arc<dyn CachePolicy>,
    /// The cut counter, mirrored out of the lock so [`snapshot`](Self::snapshot) —
    /// taken on every request — costs one atomic load.
    ///
    /// Ordering: the counter is only ever advanced while the state lock is held, and
    /// published with `Release` *after* the cut is recorded. A snapshot loading with
    /// `Acquire` therefore either sees the new value (and the recorded cut with it)
    /// or the old one — in which case the cut's sequence is greater than the
    /// snapshot and the race check catches it anyway. Both readings are safe.
    cut_seq: AtomicU64,
    /// Set the first time a failure is stored (ledger #2657) and never cleared: the
    /// resolution path looks a failure up only on a cache that has ever held one, so a
    /// kernel whose endpoints state no failure expiry pays one atomic load for it.
    failed_once: AtomicBool,
}

struct CacheState {
    entries: HashMap<CacheKey, Slot>,
    /// Failures answered with a stated expiry (ledger #2657), beside the entries and
    /// judged by the same rules: every edge at its pinned generation, the deadline
    /// in the future. Apart from `entries` so a failure is never a `CacheEntry`, never
    /// counted against the byte bound, and never handed to a [`CachePolicy`].
    failures: HashMap<CacheKey, FailedSlot>,
    /// Current generation of each golden thread (absent ⇒ generation 0).
    generations: HashMap<Thread, u64>,
    /// The most recent cuts, oldest first, as `(thread, sequence)`.
    cuts: VecDeque<(Thread, u64)>,
    /// The highest cut sequence no longer retained in `cuts`. A snapshot older than
    /// this cannot be checked, so a store against it is declined.
    horizon: u64,
    /// The cut counter (the authoritative copy; `ReprCache::cut_seq` mirrors it).
    seq: u64,
    /// A monotonic tick advanced on every store and every hit, giving the policy an
    /// insertion order and a recency order without a clock.
    tick: u64,
    /// Running total of `entries`' payload bytes.
    bytes: usize,
    /// Lifetime representation removals, excluding replacements and failures.
    evictions: u64,
}

impl Default for ReprCache {
    fn default() -> Self {
        ReprCache::new(Arc::new(Lru::default()))
    }
}

impl ReprCache {
    /// An empty cache under `policy`.
    pub fn new(policy: Arc<dyn CachePolicy>) -> Self {
        ReprCache {
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                failures: HashMap::new(),
                generations: HashMap::new(),
                cuts: VecDeque::new(),
                horizon: 0,
                seq: 0,
                tick: 0,
                bytes: 0,
                evictions: 0,
            }),
            policy,
            cut_seq: AtomicU64::new(0),
            failed_once: AtomicBool::new(false),
        }
    }

    /// The cut history as of now. Take this **before** invoking, and hand it back to
    /// [`store`](Self::store).
    pub fn snapshot(&self) -> CutSnapshot {
        CutSnapshot(self.cut_seq.load(Ordering::Acquire))
    }

    /// Serve `key` if a valid entry exists, evicting it if it has gone stale.
    /// `now` is the kernel's clock reading, or `None` on a clock-free kernel (which
    /// conservatively treats any deadline as passed).
    ///
    /// The buffer face, as 0.1.112 published it (ledger #2261): the answer is the stored
    /// buffer, copied out. An entry whose body is a [`Replay`](crate::Replay) (stored
    /// through [`store_body`](Self::store_body)) is not a buffer and is not read behind
    /// the caller's back: it answers `None` here, without counting a hit, and is served by
    /// [`get_body`](Self::get_body).
    pub fn get(&self, key: &CacheKey, now: Option<Time>) -> Option<Representation> {
        self.serve(key, now, |entry| entry.representation.copied())
    }

    /// [`get`](Self::get)'s body face (ledger #2237): a second handle on the stored body,
    /// a buffer shared without copying or a [`Replay`](crate::Replay).
    pub fn get_body(&self, key: &CacheKey, now: Option<Time>) -> Option<Representation<Body>> {
        self.serve(key, now, |entry| entry.representation.share())
    }

    /// [`get`](Self::get), in the face the kernel's caller asked for (ledger #2237): with
    /// `buffer`, an entry holding a buffer is copied out as a `Representation<Vec<u8>>`,
    /// exactly what a hit cost before bodies existed; otherwise, and for an entry holding
    /// a [`Replay`](crate::Replay), a second handle on the stored body.
    pub(crate) fn get_answer(
        &self,
        key: &CacheKey,
        now: Option<Time>,
        buffer: bool,
        incoming: Option<&Provenance>,
    ) -> Option<Answer> {
        match self.lookup(key, now, buffer, incoming, false) {
            Lookup::Hit(answer) => Some(answer),
            Lookup::Offer(_) | Lookup::Miss => None,
        }
    }

    /// [`get_answer`](Self::get_answer), with the stale answer OFFERED when the entry
    /// expired by time only and carries an origin validator (ledger #2534): the kernel
    /// invokes the endpoint with the offer, and either refreshes the entry
    /// ([`store_outcome`](Self::store_outcome) of the offered body under the new
    /// deadline) or [`discard`](Self::discard)s it. A cut entry is never offered: it is
    /// evicted, as every stale entry was before.
    pub(crate) fn get_or_offer(
        &self,
        key: &CacheKey,
        now: Option<Time>,
        buffer: bool,
        incoming: Option<&Provenance>,
        offer: &mut Option<Arc<Offer>>,
    ) -> Option<Answer> {
        match self.lookup(key, now, buffer, incoming, true) {
            Lookup::Hit(answer) => Some(answer),
            Lookup::Offer(stale) => {
                *offer = Some(stale);
                None
            }
            Lookup::Miss => None,
        }
    }

    fn lookup(
        &self,
        key: &CacheKey,
        now: Option<Time>,
        buffer: bool,
        incoming: Option<&Provenance>,
        offer: bool,
    ) -> Lookup {
        let served = self.serve_or_offer(
            key,
            now,
            |entry| {
                // Effective expiry/threads already include the upstream. They cannot
                // be reused under a different upstream, even when the bytes match.
                if entry.incoming.as_ref() != incoming {
                    return None;
                }
                let stored = &entry.representation;
                if buffer {
                    if let Some(copy) = stored.copied() {
                        return Some(Answer::Buffer(copy));
                    }
                }
                stored.share().map(Answer::Body)
            },
            |entry| {
                // An offer is an answer too: only under the upstream it was stored under.
                if !offer || entry.incoming.as_ref() != incoming {
                    return None;
                }
                let stored = entry.representation.share()?;
                let validator = stored.origin_validator()?.clone();
                Some(Arc::new(Offer {
                    stale: Arc::new(Stale {
                        validator,
                        repr_type: stored.repr_type.clone(),
                    }),
                    stored: Mutex::new(stored),
                    stored_tick: entry.stored_tick,
                }))
            },
        );
        match served {
            Served::Hit(answer) => Lookup::Hit(answer),
            Served::Offer(offer) => Lookup::Offer(offer),
            Served::Miss => Lookup::Miss,
        }
    }

    /// Forget the entry an [`Offer`] was made from, when the endpoint's answer to it was
    /// anything but "still valid" (ledger #2534): what the lookup would have done before
    /// revalidation existed. Only that entry: one stored under `key` since (a concurrent
    /// computation's) is left as it is.
    pub(crate) fn discard(&self, key: &CacheKey, offer: &Option<Arc<Offer>>) {
        let Some(offer) = offer else {
            return;
        };
        let mut state = self.state.lock().expect("cache lock");
        if state
            .entries
            .get(key)
            .is_some_and(|entry| entry.stored_tick == offer.stored_tick)
        {
            if let Some(evicted) = state.entries.remove(key) {
                state.bytes = state.bytes.saturating_sub(evicted.bytes());
                state.evictions = state.evictions.saturating_add(1);
            }
        }
    }

    /// The one serving path behind every face: a stale entry is evicted, and a valid one
    /// counts a hit only when `face` can answer from it.
    ///
    /// ★ One exception (ledger #2534): an entry expired BY TIME ONLY (every golden thread
    /// at its pinned generation) that carries an origin validator is NOT evicted. Its body
    /// and validator are what lets the endpoint ask the origin "has it changed?", so it
    /// stays until the kernel's revalidation refreshes or discards it (or the policy
    /// evicts it). It is never served as it is: a face sees only a live entry.
    fn serve<T>(
        &self,
        key: &CacheKey,
        now: Option<Time>,
        face: impl FnOnce(&Slot) -> Option<T>,
    ) -> Option<T> {
        match self.serve_or_offer(key, now, face, |_| None) {
            Served::Hit(answer) => Some(answer),
            Served::Offer(_) | Served::Miss => None,
        }
    }

    /// [`serve`](Self::serve), asking `offer` about an entry kept for revalidation.
    fn serve_or_offer<T>(
        &self,
        key: &CacheKey,
        now: Option<Time>,
        face: impl FnOnce(&Slot) -> Option<T>,
        offer: impl FnOnce(&Slot) -> Option<Arc<Offer>>,
    ) -> Served<T> {
        let mut state = self.state.lock().expect("cache lock");
        let Some(entry) = state.entries.get(key) else {
            return Served::Miss;
        };
        match state.state_of(entry, now) {
            EntryState::Live => {
                let Some(answer) = face(entry) else {
                    return Served::Miss;
                };
                state.tick += 1;
                let tick = state.tick;
                let entry = state.entries.get_mut(key).expect("entry present");
                entry.hits += 1;
                entry.last_used_tick = tick;
                Served::Hit(answer)
            }
            EntryState::Expired if entry.representation.origin_validator().is_some() => {
                offer(entry).map_or(Served::Miss, Served::Offer)
            }
            _ => {
                if let Some(evicted) = state.entries.remove(key) {
                    state.bytes = state.bytes.saturating_sub(evicted.bytes());
                    state.evictions = state.evictions.saturating_add(1);
                }
                Served::Miss
            }
        }
    }

    /// Whether `key` would be served right now — read-only: it neither evicts a
    /// stale entry nor counts as a hit, so a probe cannot perturb what a policy
    /// sees.
    pub fn probe(&self, key: &CacheKey, now: Option<Time>) -> bool {
        let state = self.state.lock().expect("cache lock");
        state
            .entries
            .get(key)
            .is_some_and(|entry| state.is_valid(entry, now))
    }

    /// Store a computed representation, unless a thread it depends on was cut since
    /// `taken` (the lost-cut race) or the policy declines to admit it. Returns
    /// whether it was stored — `false` is always a safe outcome.
    ///
    /// The buffer face, as 0.1.112 published it (ledger #2261); the buffer moves into the
    /// entry without a copy. [`store_body`](Self::store_body) is the body face.
    pub fn store(
        &self,
        key: CacheKey,
        target: String,
        representation: Representation,
        taken: CutSnapshot,
        cost_millis: Option<u64>,
    ) -> bool {
        self.store_body(key, target, representation.into_body(), taken, cost_millis)
    }

    /// [`store`](Self::store)'s body face (ledger #2237). A one-shot body
    /// ([`Body::Once`]) is never stored: `false`.
    pub fn store_body(
        &self,
        key: CacheKey,
        target: String,
        representation: Representation<Body>,
        taken: CutSnapshot,
        cost_millis: Option<u64>,
    ) -> bool {
        self.store_outcome(key, target, &representation, taken, cost_millis, None)
            .is_ok()
    }

    /// [`store`](Self::store), saying WHICH of the two declines it was — the
    /// kernel's answer to "why was this read not cached" needs the difference
    /// (`crate::uncached`).
    ///
    /// The kernel lends the answer it is about to hand back, and the entry takes a
    /// second handle on its body only once the policy has ADMITTED it. Since bodies
    /// (ledger #2237) that handle is a reference count, not a copy; before them an answer
    /// over the byte budget was cloned in full and then thrown away, so it cost twice its
    /// size (ledger #1734, measured 832 MB against 509 MB on a store dump).
    pub(crate) fn store_outcome(
        &self,
        key: CacheKey,
        target: String,
        representation: &Representation<Body>,
        taken: CutSnapshot,
        cost_millis: Option<u64>,
        incoming: Option<&Provenance>,
    ) -> Result<(), Declined> {
        // A one-shot body cannot be read again, so there is nothing to keep. The kernel
        // never offers one (it names the cause itself, `one-shot body`); this is the
        // public `store`'s answer, and the policy's word is the nearest one it has.
        if representation.bytes.is_one_shot() {
            return Err(Declined::Policy);
        }
        let mut state = self.state.lock().expect("cache lock");

        // ★ THE RACE CHECK. The representation was built from state observed before
        // any of this; if a thread it depends on moved in the meantime, what came
        // back is already stale and must not be filed against the post-cut
        // generation. Declining is free — the next read recomputes.
        if state.cut_since(representation.threads(), taken) {
            return Err(Declined::CutInFlight);
        }

        let edges: Vec<(Thread, u64)> = representation
            .threads()
            .iter()
            .map(|thread| (thread.clone(), state.generation_of(thread)))
            .collect();
        state.tick += 1;
        // The candidate's facts are exactly what `CacheEntry::facts` would report for
        // the entry below, read off the borrowed answer: nothing is copied until the
        // policy says it will be kept.
        let candidate = EntryFacts {
            request: key.request,
            target: &target,
            media_type: &representation.repr_type.media_type,
            bytes: body_size(&representation.bytes),
            cost_millis,
            hits: 0,
            threads: edges.len(),
            stored_tick: state.tick,
            last_used_tick: state.tick,
        };
        if !self.policy.admit(&candidate) {
            return Err(Declined::Policy);
        }
        // The contest (ledger #2687): when this candidate would displace a resident, the
        // policy may keep the residents instead. A replacement displaces only its own
        // key's entry, so it is not asked.
        if !state.entries.contains_key(&key) {
            let bound = self.policy.capacity();
            let over = state.entries.len() >= bound.max_entries
                || state.bytes.saturating_add(candidate.bytes) > bound.max_bytes;
            if over || self.policy.partitioned() {
                let resident: Vec<EntryFacts<'_>> = state
                    .entries
                    .iter()
                    .map(|(key, slot)| slot.facts(key.request))
                    .collect();
                if !self.policy.contest(&candidate, &resident) {
                    return Err(Declined::Policy);
                }
            }
        }
        let entry = Slot {
            incoming: incoming.cloned(),
            target,
            representation: representation.share().ok_or(Declined::Policy)?,
            edges,
            cost_millis,
            hits: 0,
            stored_tick: state.tick,
            last_used_tick: state.tick,
        };
        let bytes = entry.bytes();
        // An answer supersedes a failure cached under the same key (ledger #2657).
        state.failures.remove(&key);
        if let Some(replaced) = state.entries.insert(key, entry) {
            state.bytes = state.bytes.saturating_sub(replaced.bytes());
        }
        state.bytes = state.bytes.saturating_add(bytes);
        self.enforce_bound(&mut state);
        state.sweep_generations(self.policy.capacity().max_entries);
        Ok(())
    }

    /// Store a failure the endpoint answered with a stated expiry (ledger #2657), to be
    /// served instead of recomputing until `until` or until a cut of one of `threads`
    /// — the same two rules every entry is judged by. Declined, as an entry is, when a
    /// thread it depends on was cut since `taken`. The kernel clamps `until` to its
    /// ceiling before it gets here; the cache holds what it is given.
    #[allow(clippy::too_many_arguments)] // the key, the failure, its validity, its race check
    pub(crate) fn store_failure(
        &self,
        key: CacheKey,
        target: String,
        error: &Error,
        until: Time,
        threads: &BTreeSet<Thread>,
        taken: CutSnapshot,
        incoming: Option<&Provenance>,
    ) -> Result<(), Declined> {
        let mut state = self.state.lock().expect("cache lock");
        if state.cut_since(threads, taken) {
            return Err(Declined::CutInFlight);
        }
        let edges = threads
            .iter()
            .map(|thread| (thread.clone(), state.generation_of(thread)))
            .collect();
        if state.failures.len() >= CACHED_FAILURES && !state.failures.contains_key(&key) {
            if let Some(soonest) = state
                .failures
                .iter()
                .min_by_key(|(_, slot)| slot.until)
                .map(|(key, _)| *key)
            {
                state.failures.remove(&soonest);
            }
        }
        self.failed_once.store(true, Ordering::Release);
        state.failures.insert(
            key,
            FailedSlot {
                incoming: incoming.cloned(),
                target,
                error: error.clone(),
                until,
                threads: threads.clone(),
                edges,
            },
        );
        Ok(())
    }

    /// Whether a failure has ever been stored here — one atomic load, so the resolution
    /// path looks a failure up only on a cache that has held one.
    pub(crate) fn has_failures(&self) -> bool {
        self.failed_once.load(Ordering::Acquire)
    }

    /// The failure cached under `key`, if it is still to be served as of `now` (and was
    /// stored under the same upstream); a stale one is forgotten. `None` on a
    /// clock-free kernel, which can never tell a deadline has not passed.
    pub(crate) fn get_failure(
        &self,
        key: &CacheKey,
        now: Option<Time>,
        incoming: Option<&Provenance>,
    ) -> Option<CachedFailure> {
        let mut state = self.state.lock().expect("cache lock");
        let slot = state.failures.get(key)?;
        match state.failure_state(slot, now) {
            EntryState::Live if slot.incoming.as_ref() == incoming => Some(CachedFailure {
                error: slot.error.clone(),
                until: slot.until,
                threads: slot.threads.clone(),
            }),
            EntryState::Live => None,
            _ => {
                state.failures.remove(key);
                None
            }
        }
    }

    /// One row per cached failure, marked as of `now`. Read-only, like
    /// [`rows_at`](Self::rows_at).
    pub(crate) fn failure_rows_at(&self, now: Option<Time>) -> Vec<FailureRow> {
        let state = self.state.lock().expect("cache lock");
        let mut rows: Vec<FailureRow> = state
            .failures
            .values()
            .map(|slot| FailureRow {
                target: slot.target.clone(),
                error: slot.error.clone(),
                until: slot.until,
                state: state.failure_state(slot, now),
            })
            .collect();
        rows.sort_by(|a, b| a.until.cmp(&b.until).then_with(|| a.target.cmp(&b.target)));
        rows
    }

    /// Cut a golden thread: bump its generation (invalidating every entry pinned to
    /// an earlier one) and record the cut so an in-flight request cannot file a
    /// result that predates it.
    pub fn cut(&self, thread: Thread) {
        let mut state = self.state.lock().expect("cache lock");
        let seq = state.record_cut(thread);
        // Published last: a snapshot that sees this value has seen the record above.
        self.cut_seq.store(seq, Ordering::Release);
    }

    /// [`cut`](Self::cut), and while the lock is still held, hand `observe` the cut's
    /// sequence and every resident entry that depended on the thread — the kernel's
    /// cut listeners (`crate::listen`). Observed under the lock so two concurrent cuts
    /// reach every listener in sequence order; `observe` must therefore take no lock
    /// the cache's own callers might hold, and must not call back into the kernel.
    ///
    /// Costs one pass over the resident entries' edges, which is why the kernel calls
    /// it only when some listener's spec matches the thread.
    pub(crate) fn cut_with<R>(
        &self,
        thread: Thread,
        observe: impl FnOnce(u64, &[Dependent<'_>]) -> R,
    ) -> R {
        let mut state = self.state.lock().expect("cache lock");
        let seq = state.record_cut(thread.clone());
        self.cut_seq.store(seq, Ordering::Release);
        // Judged against the generation BEFORE this cut: `record_cut` just moved it.
        let dependents = state.dependents_before(&thread);
        observe(seq, &dependents)
    }

    /// The number of entries currently held.
    pub fn len(&self) -> usize {
        self.state.lock().expect("cache lock").entries.len()
    }

    /// The scope fingerprints of the entries currently held — what the kernel's
    /// memory of rendered chains may keep (`Kernel::scope_names`).
    pub(crate) fn resident_scopes(&self) -> std::collections::HashSet<u64> {
        let state = self.state.lock().expect("cache lock");
        state.entries.keys().map(|key| key.scope).collect()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total payload bytes currently held.
    pub fn bytes(&self) -> usize {
        self.state.lock().expect("cache lock").bytes
    }

    /// Lifetime count of representation entries removed to enforce a bound or
    /// discard stale data (including discarded revalidation offers). Saturates at
    /// `u64::MAX`, so it never decreases. Replacements, declined admissions and
    /// cached failures are excluded. Probes and readouts do not change it.
    pub fn evictions(&self) -> u64 {
        self.state.lock().expect("cache lock").evictions
    }

    /// The bound in force, for the operator readout.
    pub fn bound(&self) -> CacheBound {
        self.policy.capacity()
    }

    /// The policy in force, as it describes itself ([`CachePolicy::describe`]), for the
    /// operator readout.
    pub fn policy_description(&self) -> String {
        self.policy.describe()
    }

    /// One row per entry for `urn:kernel:cache`, with the resolution chain each
    /// was computed in — as the key's scope fingerprint, which is all the cache
    /// holds; the kernel maps it back to a rendered chain for the readout.
    ///
    /// With no clock reading, so every entry with a deadline reads
    /// [`Expired`](EntryState::Expired) — the conservative answer serving gives too.
    /// [`rows_at`](Self::rows_at) takes the reading.
    pub fn rows_with_scope(&self) -> Vec<CacheRow> {
        self.rows_at(None)
    }

    /// [`rows_with_scope`](Self::rows_with_scope), each row marked with whether it
    /// would be served as of `now` ([`EntryState`]). Read-only, like
    /// [`probe`](Self::probe): a stale row is reported, not evicted, so reading the
    /// readout cannot change what it reads (ledger #612).
    pub fn rows_at(&self, now: Option<Time>) -> Vec<CacheRow> {
        let state = self.state.lock().expect("cache lock");
        state
            .entries
            .iter()
            .map(|(key, entry)| CacheRow {
                target: entry.target.clone(),
                media_type: entry.representation.repr_type.media_type.clone(),
                bytes: entry.bytes(),
                threads: entry.edges.len(),
                scope: key.scope,
                state: state.state_of(entry, now),
            })
            .collect()
    }

    /// Every resident entry with its key and the NAMES of the golden threads it hangs
    /// from, marked as of `now` — what the graph faces of `urn:kernel:cache` and
    /// `urn:kernel:dependents` read. Read-only, like [`rows_at`](Self::rows_at).
    pub(crate) fn edges_at(&self, now: Option<Time>) -> Vec<EntryEdges> {
        let state = self.state.lock().expect("cache lock");
        state
            .entries
            .iter()
            .map(|(key, entry)| EntryEdges {
                key: *key,
                row: CacheRow {
                    target: entry.target.clone(),
                    media_type: entry.representation.repr_type.media_type.clone(),
                    bytes: entry.bytes(),
                    threads: entry.edges.len(),
                    scope: key.scope,
                    state: state.state_of(entry, now),
                },
                threads: entry
                    .edges
                    .iter()
                    .map(|(thread, _)| thread.as_str().to_string())
                    .collect(),
            })
            .collect()
    }

    /// What a cut of `thread` would recompute, as of `now`: every resident entry
    /// that hangs from it AND would be served right now. A live entry's every edge
    /// stands at its pinned generation, so its edge on `thread` is current and a cut
    /// makes it stale; an entry already stale (cut by another thread, or expired) is
    /// recomputed on its next read whatever happens to `thread`, so a cut changes
    /// nothing for it and it is not listed. Read-only.
    pub(crate) fn dependents_at(&self, thread: &Thread, now: Option<Time>) -> Vec<EntryEdges> {
        self.edges_at(now)
            .into_iter()
            .filter(|entry| {
                entry.row.state.is_live() && entry.threads.iter().any(|t| t == thread.as_str())
            })
            .collect()
    }

    /// How many times `thread` has been cut, as tracked (`0` for a thread never cut,
    /// or one swept because nothing pinned it — see `sweep_generations`).
    pub(crate) fn generation(&self, thread: &Thread) -> u64 {
        self.state.lock().expect("cache lock").generation_of(thread)
    }

    /// One row per entry for `urn:kernel:cache`: the IRI it was resolved from, its
    /// media type, its size in bytes, and how many golden threads it depends on.
    /// [`rows_with_scope`](Self::rows_with_scope) adds the chain.
    pub fn rows(&self) -> Vec<(String, String, usize, usize)> {
        let state = self.state.lock().expect("cache lock");
        state
            .entries
            .values()
            .map(|entry| {
                (
                    entry.target.clone(),
                    entry.representation.repr_type.media_type.clone(),
                    entry.bytes(),
                    entry.edges.len(),
                )
            })
            .collect()
    }

    /// One row per tracked thread for `urn:kernel:threads`: its name and how many
    /// times it has been cut. A thread no live entry depends on may have been swept,
    /// so this is the *tracked* history, not the total one.
    pub fn generation_rows(&self) -> Vec<(String, u64)> {
        let state = self.state.lock().expect("cache lock");
        state
            .generations
            .iter()
            .map(|(thread, generation)| (thread.as_str().to_string(), *generation))
            .collect()
    }

    /// Evict until the policy's bound is met (and, for a partitioned policy, every
    /// partition's), or until the policy declines to name a victim.
    fn enforce_bound(&self, state: &mut CacheState) {
        let bound = self.policy.capacity();
        let partitioned = self.policy.partitioned();
        loop {
            let over = state.entries.len() > bound.max_entries || state.bytes > bound.max_bytes;
            if !over && !partitioned {
                return;
            }
            let keys: Vec<CacheKey> = state.entries.keys().copied().collect();
            let facts: Vec<EntryFacts<'_>> = keys
                .iter()
                .map(|key| state.entries[key].facts(key.request))
                .collect();
            if !over && !self.policy.over_partition(&facts) {
                return;
            }
            let Some(index) = self.policy.victim(&facts) else {
                return;
            };
            let Some(key) = keys.get(index).copied() else {
                return;
            };
            match state.entries.remove(&key) {
                Some(evicted) => {
                    state.bytes = state.bytes.saturating_sub(evicted.bytes());
                    state.evictions = state.evictions.saturating_add(1);
                }
                None => return,
            }
        }
    }
}

/// One resident entry with its key and its golden threads by name — the graph
/// faces' view ([`ReprCache::edges_at`]).
#[derive(Clone, Debug)]
pub(crate) struct EntryEdges {
    /// The entry's key: what it is stored under.
    pub(crate) key: CacheKey,
    /// The facts the text readout prints.
    pub(crate) row: CacheRow,
    /// The threads it hangs from, by name.
    pub(crate) threads: Vec<String>,
}

/// One resident entry that depended on a thread being cut, as the kernel's cut
/// listeners see it (`crate::listen`). Borrowed from the entry: it lives only as
/// long as the cut holds the cache lock, so building it allocates nothing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Dependent<'a> {
    /// The IRI the entry was resolved from.
    pub(crate) target: &'a str,
    /// The fingerprint of the capability it was computed under — the partition a
    /// listener's authority is matched against.
    pub(crate) capability: u64,
    /// Whether THIS cut invalidated it: its edge on the thread was pinned at the
    /// generation this cut moved, so it was stored since the thread's previous cut.
    /// An entry stored before that previous cut is still a dependent (its reader was
    /// handed the thread's name) but this cut changed nothing for it. Judged on the
    /// cut thread's edge alone, deliberately: checking every other edge would cost a
    /// generation lookup per edge per dependent under the lock, and an entry some
    /// OTHER thread already made stale is listed as invalidated here, which only
    /// ever over-reports something that needs recomputing anyway.
    pub(crate) live: bool,
}

impl CacheState {
    /// Bump `thread`'s generation and log the cut; returns its sequence.
    fn record_cut(&mut self, thread: Thread) -> u64 {
        *self.generations.entry(thread.clone()).or_insert(0) += 1;
        self.seq += 1;
        let seq = self.seq;
        if self.cuts.len() >= CUT_LOG {
            if let Some((_, forgotten)) = self.cuts.pop_front() {
                self.horizon = forgotten;
            }
        }
        self.cuts.push_back((thread, seq));
        seq
    }

    /// Every resident entry with an edge on `thread`, judged against the generation
    /// the thread held before the cut just recorded (`record_cut` bumped it by one): one
    /// pass over the edges, one generation lookup in all, no allocation per entry.
    fn dependents_before(&self, thread: &Thread) -> Vec<Dependent<'_>> {
        let current = self.generation_of(thread).saturating_sub(1);
        self.entries
            .iter()
            .filter_map(|(key, entry)| {
                let (_, pinned) = entry.edges.iter().find(|(t, _)| t == thread)?;
                Some(Dependent {
                    target: &entry.target,
                    capability: key.capability,
                    live: *pinned == current,
                })
            })
            .collect()
    }

    fn generation_of(&self, thread: &Thread) -> u64 {
        self.generations.get(thread).copied().unwrap_or(0)
    }

    /// Whether an entry is valid *right now*: every golden-thread edge is still at
    /// its pinned generation, and any deadline is still in the future. The single
    /// source of truth behind both [`ReprCache::get`] and [`ReprCache::probe`], so
    /// the serving path and the read-only probe can never disagree.
    fn is_valid(&self, entry: &Slot, now: Option<Time>) -> bool {
        self.state_of(entry, now).is_live()
    }

    /// [`is_valid`](Self::is_valid), saying which half failed — a cut edge is
    /// reported ahead of a passed deadline, since the cut is the one an edit caused.
    fn state_of(&self, entry: &Slot, now: Option<Time>) -> EntryState {
        let edges_current = entry
            .edges
            .iter()
            .all(|(thread, generation)| self.generation_of(thread) == *generation);
        if !edges_current {
            return EntryState::Cut;
        }
        match entry.representation.expiry {
            Expiry::At(deadline) if !now.is_some_and(|now| now < deadline) => EntryState::Expired,
            _ => EntryState::Live,
        }
    }

    /// [`state_of`](Self::state_of) for a cached failure: cut first, then its
    /// deadline, which a clock-free kernel always reads as passed.
    fn failure_state(&self, slot: &FailedSlot, now: Option<Time>) -> EntryState {
        if !slot
            .edges
            .iter()
            .all(|(thread, generation)| self.generation_of(thread) == *generation)
        {
            return EntryState::Cut;
        }
        if now.is_some_and(|now| now < slot.until) {
            EntryState::Live
        } else {
            EntryState::Expired
        }
    }

    /// Whether any of `threads` was cut after `taken` — or whether the log can no
    /// longer answer, in which case the answer is yes.
    fn cut_since(&self, threads: &BTreeSet<Thread>, taken: CutSnapshot) -> bool {
        if threads.is_empty() {
            // Nothing to invalidate: a thread-free entry (pure computation) cannot
            // be raced by a cut at all.
            return false;
        }
        if taken.0 < self.horizon {
            return true;
        }
        self.cuts
            .iter()
            .rev()
            .take_while(|(_, seq)| *seq > taken.0)
            .any(|(thread, _)| threads.contains(thread))
    }

    /// Forget the generations of threads no resident entry depends on.
    ///
    /// The generation map is keyed by thread NAME, and a name can derive from caller
    /// input (a mutating verb auto-cuts a thread named after its target), so it is
    /// the same exhaustion vector as the entry map with a different key. Forgetting
    /// is safe exactly when nothing pins the thread: a generation number is only ever
    /// compared against an entry's pin, so a thread nobody pins can start again from
    /// zero without making any entry look fresher than it is. Pins are taken under
    /// this same lock, so no in-flight request can pin across a sweep — and the race
    /// check reads the cut log, not this map.
    fn sweep_generations(&mut self, max_entries: usize) {
        let watermark = max_entries.saturating_mul(2).max(1024);
        if self.generations.len() <= watermark {
            return;
        }
        let live: HashSet<Thread> = self
            .entries
            .values()
            .flat_map(|entry| entry.edges.iter().map(|(thread, _)| thread.clone()))
            .chain(
                self.failures
                    .values()
                    .flat_map(|slot| slot.edges.iter().map(|(thread, _)| thread.clone())),
            )
            .collect();
        self.generations.retain(|thread, _| live.contains(thread));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iri::Iri;
    use crate::repr::ReprType;
    use crate::request::Request;
    use crate::verb::Verb;

    fn key(n: u8) -> CacheKey {
        let request = Request::new(
            Verb::Source,
            Iri::parse(format!("urn:test:{n}")).expect("iri"),
        );
        CacheKey::new(request.id(), 0)
    }

    fn repr(bytes: usize) -> Representation {
        Representation::new(ReprType::new("text/plain"), vec![b'x'; bytes]).cacheable()
    }

    fn threaded(bytes: usize, thread: &str) -> Representation {
        repr(bytes).depends_on(thread)
    }

    #[test]
    fn kernel_hits_require_the_same_incoming_provenance_for_both_faces() {
        let cache = ReprCache::default();
        let up = Provenance::new(Expiry::Never, BTreeSet::from([Thread::new("urn:up:a")]));
        let other = Provenance::new(Expiry::Never, BTreeSet::from([Thread::new("urn:up:b")]));
        cache
            .store_outcome(
                key(1),
                "urn:test:1".into(),
                &repr(8).into_body(),
                cache.snapshot(),
                None,
                Some(&up),
            )
            .unwrap();
        for buffer in [false, true] {
            assert!(cache.get_answer(&key(1), None, buffer, Some(&up)).is_some());
            assert!(cache
                .get_answer(&key(1), None, buffer, Some(&other))
                .is_none());
            assert!(cache.get_answer(&key(1), None, buffer, None).is_none());
        }
        let state = cache.state.lock().unwrap();
        assert_eq!(state.entries[&key(1)].hits, 2, "mismatches are not hits");
    }

    #[test]
    fn a_cut_after_the_snapshot_declines_the_store() {
        let cache = ReprCache::default();
        let taken = cache.snapshot();
        cache.cut(Thread::new("urn:data:state"));
        assert!(
            !cache.store(
                key(1),
                "urn:test:a".into(),
                threaded(8, "urn:data:state"),
                taken,
                None
            ),
            "a result predating the cut must not be stored"
        );
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn a_cut_to_an_unrelated_thread_still_stores() {
        // The check is per-thread on purpose: a busy host cutting other resources
        // must not stop an expensive result from ever being cached.
        let cache = ReprCache::default();
        let taken = cache.snapshot();
        cache.cut(Thread::new("urn:data:elsewhere"));
        assert!(cache.store(
            key(1),
            "urn:test:a".into(),
            threaded(8, "urn:data:state"),
            taken,
            None
        ));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_snapshot_older_than_the_cut_log_declines() {
        let cache = ReprCache::default();
        let taken = cache.snapshot();
        for n in 0..(CUT_LOG + 1) {
            cache.cut(Thread::new(format!("urn:data:{n}")));
        }
        assert!(
            !cache.store(
                key(1),
                "urn:test:a".into(),
                threaded(8, "urn:data:state"),
                taken,
                None
            ),
            "cuts it can no longer see must be assumed hostile"
        );
    }

    #[test]
    fn a_thread_free_entry_is_never_raced() {
        let cache = ReprCache::default();
        let taken = cache.snapshot();
        cache.cut(Thread::new("urn:data:state"));
        assert!(
            cache.store(key(1), "urn:test:a".into(), repr(8), taken, None),
            "no threads ⇒ no cut can invalidate it"
        );
    }

    /// A key and target per name, for workloads wider than `key`'s 256.
    fn named(name: &str) -> (CacheKey, String) {
        let request = Request::new(Verb::Source, Iri::parse(name.to_string()).expect("iri"));
        (CacheKey::new(request.id(), 0), name.to_string())
    }

    /// Read `name` through `cache` as the kernel does: serve a hit, or compute and
    /// store. Answers whether it was a hit.
    fn read_through(cache: &ReprCache, name: &str, bytes: usize) -> bool {
        let (key, target) = named(name);
        if cache.get(&key, None).is_some() {
            return true;
        }
        cache.store(key, target, repr(bytes), cache.snapshot(), None);
        false
    }

    /// The feed scale round's shape (ikigai-devtools `claude/scale/register.md` entry
    /// 11, as ledger #2680 reports it): each mapped feed holds about six entries and
    /// 219 KB, and a river read walks every feed once, in order.
    const ENTRIES_PER_FEED: usize = 6;
    const ENTRY_BYTES: usize = 219_000 / ENTRIES_PER_FEED;

    /// One river read over `feeds` feeds; answers (hits, reads).
    fn river(cache: &ReprCache, feeds: usize) -> (usize, usize) {
        let mut hits = 0;
        for feed in 0..feeds {
            for part in 0..ENTRIES_PER_FEED {
                hits += usize::from(read_through(
                    cache,
                    &format!("urn:feed:{feed}/{part}"),
                    ENTRY_BYTES,
                ));
            }
        }
        (hits, feeds * ENTRIES_PER_FEED)
    }

    /// A REPL's working set: `size` entries, each read `reads` times in a row.
    fn repl(cache: &ReprCache, size: usize, reads: usize) -> (usize, usize) {
        let mut hits = 0;
        for _ in 0..reads {
            for n in 0..size {
                hits += usize::from(read_through(cache, &format!("urn:repl:{n}"), ENTRY_BYTES));
            }
        }
        (hits, size * reads)
    }

    fn percent((hits, reads): (usize, usize)) -> usize {
        hits * 100 / reads.max(1)
    }

    /// The river's hit rate, in percent, over rounds 3 to 6 (warm), under `policy`.
    fn river_rate(policy: Arc<dyn CachePolicy>, feeds: usize) -> usize {
        let cache = ReprCache::new(policy);
        let mut total = (0, 0);
        for round in 0..6 {
            let (hits, reads) = river(&cache, feeds);
            if round >= 2 {
                total = (total.0 + hits, total.1 + reads);
            }
        }
        percent(total)
    }

    /// The measurement behind ledger #2687, on the default bound (4096 entries, 64 MiB):
    /// below the cliff both policies serve the river from the cache; past it LRU serves
    /// nothing and the scan-resistant policy keeps a stable part of the river resident.
    /// `cargo test -p ikigai-core feed_round -- --nocapture` prints the table.
    #[test]
    fn feed_round_river_lru_against_scan_resistant() {
        println!("feeds  lru  scan-resistant   (warm river hit rate, %)");
        let mut rates = Vec::new();
        for feeds in [250, 350, 700] {
            let lru = river_rate(Arc::new(Lru::default()), feeds);
            let scan = river_rate(Arc::new(ScanResistant::default()), feeds);
            println!("{feeds:>5}  {lru:>3}  {scan:>14}");
            rates.push((feeds, lru, scan));
        }
        for (feeds, lru, scan) in rates {
            if feeds * 219_000 <= 64 << 20 {
                assert_eq!((lru, scan), (100, 100), "{feeds} feeds fit either way");
            } else {
                assert_eq!(
                    lru, 0,
                    "{feeds} feeds: LRU loses the whole river to the scan"
                );
                assert!(
                    scan > 0,
                    "{feeds} feeds: some of the river stays resident ({scan}%)"
                );
            }
        }
    }

    /// The test the ledger asked for (#2687): a REPL works a hot set, a river read
    /// scans past it, and the REPL comes back. LRU has evicted the hot set; the
    /// scan-resistant policy has not.
    #[test]
    fn a_scan_evicts_the_hot_set_under_lru_and_not_under_scan_resistant() {
        let run = |policy: Arc<dyn CachePolicy>| {
            let cache = ReprCache::new(policy);
            // The REPL warms up: 200 entries (about 7 MB), each read a few times.
            repl(&cache, 200, 3);
            let mut after = (0, 0);
            for _ in 0..4 {
                river(&cache, 500); // 3000 entries, about 110 MB: wider than the cache
                let (hits, reads) = repl(&cache, 200, 1);
                after = (after.0 + hits, after.1 + reads);
            }
            percent(after)
        };
        let lru = run(Arc::new(Lru::default()));
        let scan = run(Arc::new(ScanResistant::default()));
        println!("hot set after each river read: lru {lru}%, scan-resistant {scan}%");
        assert_eq!(lru, 0, "under LRU every river read flushes the hot set");
        assert_eq!(
            scan, 100,
            "under the scan-resistant policy the hot set stays"
        );
    }

    #[test]
    fn unique_queries_to_a_hot_endpoint_do_not_evict_the_working_set() {
        let cache = ReprCache::new(Arc::new(ScanResistant::with_bound(CacheBound::new(
            128,
            16 * 8,
        ))));
        let query = |n: u64| {
            let request = Request::new(Verb::Source, Iri::parse("urn:query").unwrap())
                .with_arg("query", crate::ArgRef::Inline(n.to_le_bytes().to_vec()));
            CacheKey::new(request.id(), 0)
        };
        // Fill the byte budget, with a sketch large enough to avoid incidental
        // collisions. Multiple queries make one endpoint popular, but each request
        // is distinct. Keep the older entries probationary to exercise admission.
        let mut working_set = Vec::new();
        for n in 0..8 {
            let (key, target) = named(&format!("urn:working:{n}"));
            assert!(cache.store(key, target, repr(8), cache.snapshot(), None));
            working_set.push(key);
        }
        for n in 0..8 {
            let key = query(n);
            assert!(cache.store(key, "urn:query".into(), repr(8), cache.snapshot(), None));
            working_set.push(key);
        }
        // A one-off scan must earn admission per request, not inherit the endpoint's
        // popularity and displace the other resident computations.
        for n in 8..40 {
            cache.store(
                query(n),
                "urn:query".into(),
                repr(8),
                cache.snapshot(),
                None,
            );
        }
        assert_eq!(
            working_set
                .iter()
                .filter(|key| cache.probe(key, None))
                .count(),
            working_set.len(),
            "unique queries must leave the working set resident"
        );
    }

    #[test]
    fn scan_resistant_admits_a_name_the_second_time_it_is_wanted() {
        let cache = ReprCache::new(Arc::new(ScanResistant::with_bound(CacheBound::new(
            2,
            1 << 20,
        ))));
        assert!(!read_through(&cache, "urn:test:a", 8));
        assert!(!read_through(&cache, "urn:test:b", 8));
        // Full: a name wanted once does not displace a resident wanted as often.
        let (c, target) = named("urn:test:c");
        assert!(!cache.store(c, target.clone(), repr(8), cache.snapshot(), None));
        assert_eq!(cache.evictions(), 0, "a declined candidate evicts nothing");
        // Wanted again, it has earned a place: the probationary LRU entry gives way.
        assert!(cache.store(c, target, repr(8), cache.snapshot(), None));
        assert!(cache.probe(&c, None));
        assert!(
            !cache.probe(&named("urn:test:a").0, None),
            "a was the LRU newcomer"
        );
        assert!(cache.probe(&named("urn:test:b").0, None));
    }

    #[test]
    fn scan_resistant_lets_a_working_set_move_on() {
        // Protected entries may hold 50% of four entries: two.
        let cache = ReprCache::new(Arc::new(
            ScanResistant::with_bound(CacheBound::new(4, 1 << 20)).protecting(50),
        ));
        for name in ["urn:old:a", "urn:old:b", "urn:old:c", "urn:old:d"] {
            read_through(&cache, name, 8);
            assert!(
                read_through(&cache, name, 8),
                "{name} is served and protected"
            );
        }
        // A new working set, each name wanted twice and then served, displaces the old.
        for _ in 0..4 {
            for name in ["urn:new:a", "urn:new:b", "urn:new:c", "urn:new:d"] {
                read_through(&cache, name, 8);
            }
        }
        let resident: Vec<String> = cache.rows().into_iter().map(|row| row.0).collect();
        assert!(
            resident
                .iter()
                .filter(|name| name.starts_with("urn:new:"))
                .count()
                >= 3,
            "the new working set took over: {resident:?}"
        );
    }

    #[test]
    fn a_partition_holds_its_budget_while_another_fills() {
        let cache = ReprCache::new(Arc::new(
            Partitioned::new(Arc::new(Lru::with_bound(CacheBound::new(100, 400)))).partition(
                "urn:feed:",
                Arc::new(Lru::with_bound(CacheBound::new(100, 1000))),
            ),
        ));
        // The REPL (the rest) holds its working set: 4 × 100 bytes, its whole budget.
        for n in 0..4 {
            read_through(&cache, &format!("urn:repl:{n}"), 100);
        }
        // The river fills its partition many times over.
        for n in 0..50 {
            read_through(&cache, &format!("urn:feed:{n}"), 100);
        }
        let rows = cache.rows();
        let bytes = |prefix: &str| -> usize {
            rows.iter()
                .filter(|row| row.0.starts_with(prefix))
                .map(|row| row.2)
                .sum()
        };
        assert_eq!(
            bytes("urn:feed:"),
            1000,
            "the river is held to its own budget"
        );
        assert_eq!(bytes("urn:repl:"), 400, "and evicted none of the REPL's");
        for n in 0..4 {
            assert!(read_through(&cache, &format!("urn:repl:{n}"), 100));
        }
        // The budget is hard: with the REPL's partition empty, the river still stops at
        // its own.
        let cache = ReprCache::new(Arc::new(
            Partitioned::new(Arc::new(Lru::with_bound(CacheBound::new(100, 400)))).partition(
                "urn:feed:",
                Arc::new(Lru::with_bound(CacheBound::new(3, 1000))),
            ),
        ));
        for n in 0..10 {
            read_through(&cache, &format!("urn:feed:{n}"), 10);
        }
        assert_eq!(cache.len(), 3, "the partition's entry count holds too");
        assert!(
            read_through(&cache, "urn:feed:9", 10),
            "LRU within the partition"
        );
    }

    #[test]
    fn a_partition_refuses_what_its_own_budget_cannot_hold() {
        let policy = Partitioned::new(Arc::new(Lru::with_bound(CacheBound::new(10, 1000))))
            .partition(
                "urn:feed:",
                Arc::new(Lru::with_bound(CacheBound::new(10, 100))),
            );
        assert_eq!(policy.capacity(), CacheBound::new(20, 1100));
        let cache = ReprCache::new(Arc::new(policy));
        let (key, target) = named("urn:feed:big");
        assert!(!cache.store(key, target, repr(101), cache.snapshot(), None));
        let (key, target) = named("urn:repl:big");
        assert!(cache.store(key, target, repr(101), cache.snapshot(), None));
    }

    #[test]
    fn the_longest_prefix_claims_an_entry() {
        let cache = ReprCache::new(Arc::new(
            Partitioned::new(Arc::new(Lru::with_bound(CacheBound::new(10, 1000))))
                .partition(
                    "urn:feed:",
                    Arc::new(Lru::with_bound(CacheBound::new(10, 1000))),
                )
                .partition(
                    "urn:feed:hot/",
                    Arc::new(Lru::with_bound(CacheBound::new(1, 1000))),
                ),
        ));
        read_through(&cache, "urn:feed:hot/a", 8);
        read_through(&cache, "urn:feed:hot/b", 8);
        read_through(&cache, "urn:feed:cold", 8);
        assert_eq!(cache.len(), 2, "the hot partition holds one entry");
        assert!(cache.probe(&named("urn:feed:cold").0, None));
        assert!(cache.probe(&named("urn:feed:hot/b").0, None));
    }

    #[test]
    fn every_shipped_policy_describes_itself() {
        let bound = CacheBound::new(8, 64);
        assert_eq!(
            Lru::with_bound(bound).describe(),
            "lru (8 entries, 64 bytes)"
        );
        assert_eq!(
            Fifo::with_bound(bound).describe(),
            "fifo (8 entries, 64 bytes)"
        );
        assert_eq!(
            CostAware::with_bound(bound).describe(),
            "cost-aware (8 entries, 64 bytes)"
        );
        assert_eq!(
            ScanResistant::with_bound(bound).protecting(70).describe(),
            "scan-resistant (8 entries, 64 bytes, protected 70%)"
        );
        assert_eq!(
            Partitioned::new(Arc::new(Lru::with_bound(bound)))
                .partition("urn:feed:", Arc::new(ScanResistant::with_bound(bound)))
                .describe(),
            "partitioned (urn:feed:* scan-resistant (8 entries, 64 bytes, protected 80%); \
             rest lru (8 entries, 64 bytes))"
        );
        struct Mine;
        impl CachePolicy for Mine {
            fn victim(&self, _resident: &[EntryFacts<'_>]) -> Option<usize> {
                None
            }
        }
        assert_eq!(Mine.describe(), "custom");
        assert_eq!(
            ReprCache::default().policy_description(),
            "lru (4096 entries, 67108864 bytes)"
        );
    }

    #[test]
    fn the_entry_bound_evicts() {
        let cache = ReprCache::new(Arc::new(Lru::with_bound(CacheBound::new(2, 1 << 20))));
        for n in 0..4u8 {
            assert!(cache.store(
                key(n),
                format!("urn:test:{n}"),
                repr(8),
                cache.snapshot(),
                None
            ));
        }
        assert_eq!(cache.len(), 2, "the bound is enforced on every store");
        assert_eq!(cache.evictions(), 2, "each victim counts once");
    }

    #[test]
    fn the_byte_bound_evicts_and_refuses_the_oversized() {
        let cache = ReprCache::new(Arc::new(Lru::with_bound(CacheBound::new(16, 64))));
        assert!(cache.store(
            key(1),
            "urn:test:a".into(),
            repr(40),
            cache.snapshot(),
            None
        ));
        assert!(cache.store(
            key(2),
            "urn:test:b".into(),
            repr(40),
            cache.snapshot(),
            None
        ));
        assert_eq!(cache.len(), 1, "80 bytes does not fit in 64");
        assert_eq!(cache.evictions(), 1);
        assert!(
            !cache.store(
                key(3),
                "urn:test:c".into(),
                repr(65),
                cache.snapshot(),
                None
            ),
            "an entry larger than the whole budget is refused, not admitted-then-evicted"
        );
    }

    #[test]
    fn eviction_count_excludes_replacements_rejections_and_observation() {
        let cache = ReprCache::new(Arc::new(Lru::with_bound(CacheBound::new(2, 16))));
        assert_eq!(cache.evictions(), 0);
        for _ in 0..2 {
            assert!(cache.store(key(1), "a".into(), repr(8), cache.snapshot(), None));
        }
        assert!(!cache.store(key(2), "b".into(), repr(17), cache.snapshot(), None));
        assert!(cache.get(&key(1), None).is_some());
        assert!(cache.get(&key(2), None).is_none());
        assert_eq!(cache.rows().len(), 1);
        assert_eq!(cache.evictions(), 0);
        // One large candidate requires two victims in a single store.
        assert!(cache.store(key(2), "b".into(), repr(8), cache.snapshot(), None));
        assert!(cache.store(key(3), "c".into(), repr(16), cache.snapshot(), None));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.evictions(), 2);
    }

    #[test]
    fn revalidation_counts_a_discard_once_and_preserves_a_replacement() {
        let cache = ReprCache::default();
        let validated = || {
            repr(8)
                .cacheable_until(Time::from_millis(100))
                .validated_by(crate::OriginValidator {
                    etag: Some("v1".into()),
                    last_modified: None,
                })
        };
        let later = Some(Time::from_millis(100));
        assert!(cache.store(key(1), "a".into(), validated(), cache.snapshot(), None));
        let mut offer = None;
        assert!(cache
            .get_or_offer(&key(1), later, true, None, &mut offer)
            .is_none());
        assert!(offer.is_some());
        assert_eq!(cache.evictions(), 0, "offering retains the entry");
        cache.discard(&key(1), &offer);
        cache.discard(&key(1), &offer);
        assert_eq!(cache.evictions(), 1);
        assert!(cache.store(key(1), "a".into(), validated(), cache.snapshot(), None));
        assert!(cache
            .get_or_offer(&key(1), later, true, None, &mut offer)
            .is_none());
        assert!(cache.store(key(1), "a".into(), repr(8), cache.snapshot(), None));
        cache.discard(&key(1), &offer);
        assert!(cache.probe(&key(1), later), "a newer entry survives");
        assert_eq!(cache.evictions(), 1, "replacement is not an eviction");
    }

    #[test]
    fn expiry_counts_only_when_the_entry_is_removed() {
        let cache = ReprCache::default();
        let repr = repr(8).cacheable_until(Time::from_millis(100));
        assert!(cache.store(key(1), "a".into(), repr, cache.snapshot(), None));
        let now = Some(Time::from_millis(100));
        assert!(!cache.probe(&key(1), now));
        assert_eq!(cache.rows_at(now)[0].state, EntryState::Expired);
        assert_eq!(cache.evictions(), 0);
        assert!(cache.get_body(&key(1), now).is_none());
        assert_eq!(cache.evictions(), 1);
        assert!(cache.get(&key(1), now).is_none());
        assert_eq!(cache.evictions(), 1);
    }

    #[test]
    fn lru_evicts_the_least_recently_served() {
        let cache = ReprCache::new(Arc::new(Lru::with_bound(CacheBound::new(2, 1 << 20))));
        cache.store(key(1), "a".into(), repr(8), cache.snapshot(), None);
        cache.store(key(2), "b".into(), repr(8), cache.snapshot(), None);
        // Serving 1 makes 2 the least recently used.
        assert!(cache.get(&key(1), None).is_some());
        cache.store(key(3), "c".into(), repr(8), cache.snapshot(), None);
        assert!(cache.probe(&key(1), None), "recently served survives");
        assert!(
            !cache.probe(&key(2), None),
            "least recently served is evicted"
        );
    }

    #[test]
    fn fifo_evicts_the_oldest_however_recently_served() {
        let cache = ReprCache::new(Arc::new(Fifo::with_bound(CacheBound::new(2, 1 << 20))));
        cache.store(key(1), "a".into(), repr(8), cache.snapshot(), None);
        cache.store(key(2), "b".into(), repr(8), cache.snapshot(), None);
        assert!(cache.get(&key(1), None).is_some());
        cache.store(key(3), "c".into(), repr(8), cache.snapshot(), None);
        assert!(
            !cache.probe(&key(1), None),
            "oldest goes, hits notwithstanding"
        );
        assert!(cache.probe(&key(2), None));
    }

    #[test]
    fn cost_aware_keeps_the_expensive_and_compact() {
        let cache = ReprCache::new(Arc::new(CostAware::with_bound(CacheBound::new(2, 1 << 20))));
        // Cheap and large versus expensive and small.
        cache.store(key(1), "cheap".into(), repr(64), cache.snapshot(), Some(1));
        cache.store(key(2), "dear".into(), repr(8), cache.snapshot(), Some(900));
        cache.store(key(3), "new".into(), repr(8), cache.snapshot(), Some(500));
        assert!(
            !cache.probe(&key(1), None),
            "cheap-per-byte is evicted first"
        );
        assert!(cache.probe(&key(2), None));
    }

    #[test]
    fn cost_aware_without_a_clock_falls_back_to_recency() {
        let cache = ReprCache::new(Arc::new(CostAware::with_bound(CacheBound::new(2, 1 << 20))));
        cache.store(key(1), "a".into(), repr(8), cache.snapshot(), None);
        cache.store(key(2), "b".into(), repr(8), cache.snapshot(), None);
        assert!(cache.get(&key(1), None).is_some());
        cache.store(key(3), "c".into(), repr(8), cache.snapshot(), None);
        assert!(
            cache.probe(&key(1), None),
            "unmeasured cost is not zero cost"
        );
        assert!(!cache.probe(&key(2), None));
    }

    #[test]
    fn a_cut_invalidates_and_the_lookup_evicts() {
        let cache = ReprCache::default();
        assert!(cache.store(
            key(1),
            "a".into(),
            threaded(8, "urn:data:state"),
            cache.snapshot(),
            None
        ));
        cache.cut(Thread::new("urn:data:state"));
        assert!(!cache.probe(&key(1), None), "a cut entry is not served");
        assert_eq!(cache.len(), 1, "the probe does not evict");
        assert_eq!(cache.evictions(), 0);
        assert!(cache.get(&key(1), None).is_none());
        assert_eq!(
            cache.len(),
            0,
            "the serving path evicts what it found stale"
        );
        assert_eq!(cache.evictions(), 1);
    }

    #[test]
    fn the_rows_say_which_entries_a_cut_has_made_stale_without_evicting_them() {
        let cache = ReprCache::default();
        assert!(cache.store(
            key(1),
            "urn:test:a".into(),
            threaded(8, "urn:data:state"),
            cache.snapshot(),
            None
        ));
        assert!(cache.store(
            key(2),
            "urn:test:b".into(),
            threaded(8, "urn:data:other"),
            cache.snapshot(),
            None
        ));
        let deadline = Representation::new(ReprType::new("text/plain"), b"x".to_vec())
            .cacheable_until(Time::from_millis(100));
        assert!(cache.store(
            key(3),
            "urn:test:c".into(),
            deadline,
            cache.snapshot(),
            None
        ));
        cache.cut(Thread::new("urn:data:state"));
        let mut rows = cache.rows_at(Some(Time::from_millis(100)));
        rows.sort_by(|a, b| a.target.cmp(&b.target));
        let states: Vec<(&str, EntryState)> =
            rows.iter().map(|r| (r.target.as_str(), r.state)).collect();
        assert_eq!(
            states,
            [
                ("urn:test:a", EntryState::Cut),
                ("urn:test:b", EntryState::Live),
                ("urn:test:c", EntryState::Expired),
            ]
        );
        assert_eq!(cache.len(), 3, "reading the rows evicts nothing");
    }

    #[test]
    fn generations_are_swept_once_nothing_pins_them() {
        let cache = ReprCache::new(Arc::new(Lru::with_bound(CacheBound::new(4, 1 << 20))));
        // One live entry pinning one thread, at a generation worth remembering.
        // (A thread that has never been cut is absent from the map entirely — the
        // map holds cut history, so there is nothing to sweep for one.)
        cache.cut(Thread::new("urn:data:pinned"));
        assert!(cache.store(
            key(1),
            "a".into(),
            threaded(8, "urn:data:pinned"),
            cache.snapshot(),
            None
        ));
        // …and a flood of caller-named threads nothing depends on.
        for n in 0..2100 {
            cache.cut(Thread::new(format!("urn:data:flood:{n}")));
        }
        assert!(
            cache.generation_rows().len() > 2000,
            "the sweep is driven by a store, not by the cut itself"
        );
        assert!(cache.store(key(2), "b".into(), repr(8), cache.snapshot(), None));
        let rows = cache.generation_rows();
        assert_eq!(rows.len(), 1, "only pinned threads survive the sweep");
        assert_eq!(rows[0], ("urn:data:pinned".to_string(), 1));
        // …and the entry that pins it is still valid, which is what makes it safe.
        assert!(cache.probe(&key(1), None));
    }

    #[test]
    fn a_policy_declining_to_name_a_victim_leaves_the_cache_over_its_bound() {
        // The escape hatch is explicit, never accidental: a policy that wants an
        // unbounded cache has to say so here.
        struct KeepEverything;
        impl CachePolicy for KeepEverything {
            fn capacity(&self) -> CacheBound {
                CacheBound::new(1, 1)
            }
            fn admit(&self, _candidate: &EntryFacts<'_>) -> bool {
                true
            }
            fn victim(&self, _resident: &[EntryFacts<'_>]) -> Option<usize> {
                None
            }
        }
        let cache = ReprCache::new(Arc::new(KeepEverything));
        for n in 0..5u8 {
            assert!(cache.store(key(n), "a".into(), repr(8), cache.snapshot(), None));
        }
        assert_eq!(cache.len(), 5);
    }

    #[test]
    fn a_deadline_is_honored_and_a_clockless_kernel_assumes_the_worst() {
        let cache = ReprCache::default();
        let repr = Representation::new(ReprType::new("text/plain"), b"x".to_vec())
            .cacheable_until(Time::from_millis(100));
        assert!(cache.store(key(1), "a".into(), repr, cache.snapshot(), None));
        assert!(cache.probe(&key(1), Some(Time::from_millis(99))));
        assert!(!cache.probe(&key(1), Some(Time::from_millis(100))));
        assert!(
            !cache.probe(&key(1), None),
            "no clock ⇒ cannot claim unexpired"
        );
    }
}
