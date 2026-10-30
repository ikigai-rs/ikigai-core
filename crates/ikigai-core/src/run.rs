//! **The run's context: a retry budget and a deadline that ride authority** (ledger #1546,
//! #200; the retry-authority note, section 4, decisions 5, 7 and 8).
//!
//! A retry budget belongs to whoever holds the authority: the party that issued the request
//! (a door acting for a principal), minted with its authority and spent by the whole
//! resolution tree below it. Two halves carry it, and neither is the request, because a
//! [`Request`](crate::Request) is half the cache key and crosses a wire as data a peer can
//! forge:
//!
//! - **The CEILING rides the capability**: a grant [`cap_retry`]`(n)`,
//!   `urn:cap:retry:<n>`, monotone in grants exactly as `urn:cap:store:budget:<ms>` is
//!   ([`RetryLimits::for_capability`]): a holder gets the largest `n` it holds, never above
//!   the host's ceiling; root gets the ceiling; no grant gets the host's base. Attenuation
//!   can only remove the grant, so it can only LOWER the budget.
//! - **The COUNT rides the kernel's [`IssueContext`]**, threaded through every sub-request
//!   as the nesting depth is: one shared [`RetryBudget`] pool for the whole tree, so k
//!   nested retry layers are bounded by one number rather than multiplying.
//!
//! Beside the count, the same context carries the run's **deadline** (`not_after`, on the
//! kernel clock): minted by a door, readable by every endpoint below it
//! ([`Invocation::not_after`](crate::Invocation::not_after),
//! [`Invocation::time_left`](crate::Invocation::time_left)), and only ever shortened on the
//! way down. A budgeted module takes the least of its own bound and what the run has left.
//!
//! And the run's **byte budget** (ledger #2238, stage 2 of the streams design): a
//! [`MemoryBudget`] pool of the bytes the kernel may BUFFER for the whole tree, collecting
//! streamed bodies into memory. Its ceiling rides the grant [`cap_stream_memory`]`(n)`,
//! `urn:cap:stream:memory:<bytes>`, by the same monotone rule as the retry grant
//! ([`MemoryLimits::for_capability`]), and its count rides the [`IssueContext`]. Unlike
//! the retry pool it always exists: a kernel that configures nothing runs under
//! [`MemoryLimits::default`], [`DEFAULT_STREAM_MEMORY`] for every caller.
//!
//! ★ **Nothing changes for a kernel that configures nothing** (decision 5): without
//! [`Kernel::with_retry_budget`](crate::Kernel::with_retry_budget) there is no pool,
//! [`Invocation::retry_budget`](crate::Invocation::retry_budget) answers `None`, and every
//! retry overlay keeps its own `attempts`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::capability::Capability;
use crate::repr::Time;

/// The family of retry grants: `urn:cap:retry:<n>` (ledger #1546, decision 8). Not a
/// requirement of any endpoint: holding none is the ordinary case, and gets the host's base.
pub const CAP_RETRY: &str = "urn:cap:retry:*";

/// The grant that lets a resolution tree re-issue up to `retries` failed requests in all
/// (clamped to the host's ceiling): `urn:cap:retry:<retries>`.
///
/// ```
/// use ikigai_core::cap_retry;
///
/// assert_eq!(cap_retry(1), "urn:cap:retry:1");
/// ```
pub fn cap_retry(retries: u32) -> String {
    format!("{}{retries}", CAP_RETRY.trim_end_matches('*'))
}

/// A host's retry policy: the **base** every caller gets, and the **ceiling** no caller
/// exceeds and root gets (set with
/// [`Kernel::with_retry_budget`](crate::Kernel::with_retry_budget)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryLimits {
    base: u32,
    ceiling: u32,
}

impl RetryLimits {
    /// A base and a ceiling; a base above the ceiling is lowered to it.
    pub fn new(base: u32, ceiling: u32) -> Self {
        RetryLimits {
            base: base.min(ceiling),
            ceiling,
        }
    }

    /// The base every caller gets.
    pub fn base(&self) -> u32 {
        self.base
    }

    /// The most any caller gets, and what root gets.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// The retries a capability is entitled to: **root** gets the ceiling; a scoped
    /// capability gets the largest `urn:cap:retry:<n>` it holds (and does not deny), never
    /// below the base and never above the ceiling. The rule and its reason are
    /// `ikigai-store`'s for `urn:cap:store:budget:<ms>`: it is monotone in grants, so a
    /// caller cannot raise its own budget by attenuating, and a "you get less" grant would be
    /// one a caller could drop. A host that wants its callers to own every retry sets a base
    /// of 0 and grants nothing.
    ///
    /// ```
    /// use ikigai_core::{cap_retry, Capability, RetryLimits};
    ///
    /// let limits = RetryLimits::new(0, 3);
    /// assert_eq!(limits.for_capability(&Capability::root()), 3);
    /// assert_eq!(limits.for_capability(&Capability::scoped(["urn:cap:read"])), 0);
    /// let one = Capability::scoped([cap_retry(1)]);
    /// assert_eq!(limits.for_capability(&one), 1);
    /// // The largest grant held wins; nothing exceeds the ceiling.
    /// let many = Capability::scoped([cap_retry(2), cap_retry(9)]);
    /// assert_eq!(limits.for_capability(&many), 3);
    /// // A base is a floor for every caller.
    /// assert_eq!(RetryLimits::new(2, 3).for_capability(&one), 2);
    /// ```
    pub fn for_capability(&self, capability: &Capability) -> u32 {
        let Some(granted) = largest_grant::<u32>(capability, CAP_RETRY) else {
            return self.ceiling;
        };
        granted.max(self.base).min(self.ceiling)
    }
}

/// The largest numeric grant under `family` (`urn:cap:…:*`) that `capability` holds and
/// does not deny: `None` for root (which gets a ceiling, not a grant), `Some(0)` for a
/// scoped capability holding none. One rule for every budget grant, so the retry and
/// byte budgets cannot drift apart.
fn largest_grant<N>(capability: &Capability, family: &str) -> Option<N>
where
    N: std::str::FromStr + Ord + Default,
{
    let scopes = capability.scopes()?;
    let prefix = family.trim_end_matches('*');
    Some(
        scopes
            .iter()
            .filter(|scope| capability.allows(scope))
            .filter_map(|scope| scope.strip_prefix(prefix)?.parse::<N>().ok())
            .max()
            .unwrap_or_default(),
    )
}

/// The **retry pool** of one resolution tree: how many failed requests may still be
/// re-issued anywhere below the door that minted it (ledger #1546).
///
/// Shared, not copied: every sub-request of the tree draws from the same count, so a retry
/// layer inside a retry layer cannot multiply the attempts against a dependency that is
/// already failing. A narrower view ([`Invocation::with_retries`](crate::Invocation::with_retries),
/// or a sub-request whose capability is entitled to fewer) is a child pool: it has its own
/// limit AND draws from every pool above it, so a spend anywhere is a spend everywhere above
/// it. There is no way to widen one.
///
/// Minted only by the kernel, from the capability ([`RetryLimits::for_capability`]) and
/// anything the door asked for ([`IssueContext::with_retry_limit`]); an endpoint reads it
/// through [`Invocation::retry_budget`](crate::Invocation::retry_budget).
///
/// ```
/// use ikigai_core::IssueContext;
///
/// // A door that grants one retry to the whole run.
/// let run = IssueContext::new().with_retry_limit(1);
/// let pool = run.retry_budget().unwrap();
/// assert_eq!(pool.remaining(), 1);
/// assert!(pool.try_spend());
/// assert!(!pool.try_spend(), "one retry, wherever in the tree it happens");
/// assert_eq!(pool.remaining(), 0);
/// ```
#[derive(Clone)]
pub struct RetryBudget(Pool);

impl RetryBudget {
    /// A fresh pool of `retries`.
    pub(crate) fn new(retries: u32) -> Self {
        RetryBudget(Pool::new(u64::from(retries)))
    }

    /// How many re-issues are left: the least along this pool and every pool above it.
    pub fn remaining(&self) -> u32 {
        u32::try_from(self.0.remaining()).unwrap_or(u32::MAX)
    }

    /// Spend one re-issue: `true` if one was left (and is now spent, here and in every
    /// pool above), `false` if none was, in which case nothing is spent. An overlay calls
    /// this BEFORE it re-issues, and returns the error it received when it answers `false`.
    pub fn try_spend(&self) -> bool {
        self.0.try_take(1)
    }

    /// A view of this pool limited to `limit`: the same pool when `limit` is no less than
    /// what is left, else a child that draws from it.
    pub(crate) fn narrowed(&self, limit: u32) -> RetryBudget {
        RetryBudget(self.0.narrowed(u64::from(limit)))
    }
}

/// A shared, only-narrowing count: what [`RetryBudget`] and [`MemoryBudget`] are. A child
/// has its own limit AND draws from every pool above it.
#[derive(Clone)]
struct Pool(Arc<PoolCell>);

struct PoolCell {
    left: AtomicU64,
    parent: Option<Pool>,
}

impl Pool {
    fn new(amount: u64) -> Self {
        Pool(Arc::new(PoolCell {
            left: AtomicU64::new(amount),
            parent: None,
        }))
    }

    /// The least left along this pool and every pool above it.
    fn remaining(&self) -> u64 {
        let mut least = u64::MAX;
        let mut pool = Some(self);
        while let Some(p) = pool {
            least = least.min(p.0.left.load(Ordering::Acquire));
            pool = p.0.parent.as_ref();
        }
        least
    }

    /// Take `amount` here and in every pool above, or nothing at all: a pool that cannot
    /// cover it gives back what this call took below it.
    fn try_take(&self, amount: u64) -> bool {
        let mut taken: Vec<&Pool> = Vec::new();
        let mut pool = Some(self);
        while let Some(p) = pool {
            if !take(&p.0.left, amount) {
                for p in taken {
                    p.0.left.fetch_add(amount, Ordering::AcqRel);
                }
                return false;
            }
            taken.push(p);
            pool = p.0.parent.as_ref();
        }
        true
    }

    /// Return `amount` taken by [`try_take`](Self::try_take), here and in every pool above.
    fn give_back(&self, amount: u64) {
        let mut pool = Some(self);
        while let Some(p) = pool {
            p.0.left.fetch_add(amount, Ordering::AcqRel);
            pool = p.0.parent.as_ref();
        }
    }

    fn narrowed(&self, limit: u64) -> Pool {
        if limit >= self.remaining() {
            return self.clone();
        }
        Pool(Arc::new(PoolCell {
            left: AtomicU64::new(limit),
            parent: Some(self.clone()),
        }))
    }

    #[cfg(test)]
    fn same(&self, other: &Pool) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Subtract `amount` from `left` if it covers it: a compare-and-swap loop, so a take that
/// finds too little takes nothing.
fn take(left: &AtomicU64, amount: u64) -> bool {
    let mut current = left.load(Ordering::Acquire);
    while let Some(next) = current.checked_sub(amount) {
        match left.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(seen) => current = seen,
        }
    }
    false
}

/// The family of byte-budget grants: `urn:cap:stream:memory:<bytes>` (ledger #2238). Like
/// [`CAP_RETRY`], required by no endpoint: holding none is the ordinary case, and gets the
/// host's base.
pub const CAP_STREAM_MEMORY: &str = "urn:cap:stream:memory:*";

/// The grant that lets a resolution tree buffer up to `bytes` in all (clamped to the
/// host's ceiling): `urn:cap:stream:memory:<bytes>`.
///
/// ```
/// use ikigai_core::cap_stream_memory;
///
/// assert_eq!(cap_stream_memory(1_048_576), "urn:cap:stream:memory:1048576");
/// ```
pub fn cap_stream_memory(bytes: u64) -> String {
    format!("{}{bytes}", CAP_STREAM_MEMORY.trim_end_matches('*'))
}

/// The byte budget of a kernel that configures none: 64 MiB, base and ceiling, the wire's
/// `MAX_FRAME`, so it refuses no streamed body that a wire could have carried whole.
pub const DEFAULT_STREAM_MEMORY: u64 = 64 * 1024 * 1024;

/// A host's byte-budget policy: the **base** every caller gets and the **ceiling** no
/// caller exceeds and root gets (set with
/// [`Kernel::with_stream_memory`](crate::Kernel::with_stream_memory)). The default is
/// [`DEFAULT_STREAM_MEMORY`] for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryLimits {
    base: u64,
    ceiling: u64,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        MemoryLimits::new(DEFAULT_STREAM_MEMORY, DEFAULT_STREAM_MEMORY)
    }
}

impl MemoryLimits {
    /// A base and a ceiling, in bytes; a base above the ceiling is lowered to it.
    pub fn new(base: u64, ceiling: u64) -> Self {
        MemoryLimits {
            base: base.min(ceiling),
            ceiling,
        }
    }

    /// The bytes every caller may buffer.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The most any caller may buffer, and what root gets.
    pub fn ceiling(&self) -> u64 {
        self.ceiling
    }

    /// The bytes a capability may buffer, by [`RetryLimits::for_capability`]'s rule:
    /// **root** gets the ceiling; a scoped capability gets the largest
    /// `urn:cap:stream:memory:<bytes>` it holds (and does not deny), never below the base
    /// and never above the ceiling. Monotone in grants, so attenuating can only lower it.
    ///
    /// ```
    /// use ikigai_core::{cap_stream_memory, Capability, MemoryLimits};
    ///
    /// let limits = MemoryLimits::new(1_024, 1 << 20);
    /// assert_eq!(limits.for_capability(&Capability::root()), 1 << 20);
    /// assert_eq!(limits.for_capability(&Capability::scoped(["urn:cap:read"])), 1_024);
    /// let more = Capability::scoped([cap_stream_memory(65_536)]);
    /// assert_eq!(limits.for_capability(&more), 65_536);
    /// let huge = Capability::scoped([cap_stream_memory(u64::MAX)]);
    /// assert_eq!(limits.for_capability(&huge), 1 << 20, "never above the ceiling");
    /// ```
    pub fn for_capability(&self, capability: &Capability) -> u64 {
        let Some(granted) = largest_grant::<u64>(capability, CAP_STREAM_MEMORY) else {
            return self.ceiling;
        };
        granted.max(self.base).min(self.ceiling)
    }
}

/// The family of disk-budget grants: `urn:cap:stream:spill:<bytes>` (ledger #2372), the
/// bytes a caller may have `urn:stream:spill` write to the host's spill facility for one
/// request. Unlike [`CAP_STREAM_MEMORY`] it is REQUIRED: `urn:stream:spill` declares the
/// wildcard, so a caller holding no grant in the family is denied before anything is written.
pub const CAP_STREAM_SPILL: &str = "urn:cap:stream:spill:*";

/// The grant that lets one `urn:stream:spill` request write up to `bytes` (clamped to the
/// host's ceiling): `urn:cap:stream:spill:<bytes>`.
///
/// ```
/// use ikigai_core::cap_stream_spill;
///
/// assert_eq!(cap_stream_spill(1_048_576), "urn:cap:stream:spill:1048576");
/// ```
pub fn cap_stream_spill(bytes: u64) -> String {
    format!("{}{bytes}", CAP_STREAM_SPILL.trim_end_matches('*'))
}

/// The disk ceiling of a kernel that configures none: 1 GiB for one spill.
pub const DEFAULT_STREAM_SPILL: u64 = 1024 * 1024 * 1024;

/// A host's disk-budget policy for `urn:stream:spill` (ledger #2372): the **base** every
/// granted caller gets and the **ceiling** no caller exceeds and root gets (set with
/// [`Kernel::with_stream_spill`](crate::Kernel::with_stream_spill)). The default is a base
/// of 0 and a ceiling of [`DEFAULT_STREAM_SPILL`], so a grant is exactly what its holder
/// may write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpillLimits {
    base: u64,
    ceiling: u64,
}

impl Default for SpillLimits {
    fn default() -> Self {
        SpillLimits::new(0, DEFAULT_STREAM_SPILL)
    }
}

impl SpillLimits {
    /// A base and a ceiling, in bytes; a base above the ceiling is lowered to it.
    pub fn new(base: u64, ceiling: u64) -> Self {
        SpillLimits {
            base: base.min(ceiling),
            ceiling,
        }
    }

    /// The bytes every granted caller may spill.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The most any caller may spill, and what root gets.
    pub fn ceiling(&self) -> u64 {
        self.ceiling
    }

    /// The bytes a capability may spill, by [`MemoryLimits::for_capability`]'s rule:
    /// **root** gets the ceiling; a scoped capability gets the largest
    /// `urn:cap:stream:spill:<bytes>` it holds (and does not deny), never below the base
    /// and never above the ceiling. Monotone in grants, so attenuating can only lower it.
    ///
    /// ```
    /// use ikigai_core::{cap_stream_spill, Capability, SpillLimits};
    ///
    /// let limits = SpillLimits::new(0, 1 << 20);
    /// assert_eq!(limits.for_capability(&Capability::root()), 1 << 20);
    /// assert_eq!(limits.for_capability(&Capability::scoped(["urn:cap:read"])), 0);
    /// let some = Capability::scoped([cap_stream_spill(65_536)]);
    /// assert_eq!(limits.for_capability(&some), 65_536);
    /// let huge = Capability::scoped([cap_stream_spill(u64::MAX)]);
    /// assert_eq!(limits.for_capability(&huge), 1 << 20, "never above the ceiling");
    /// ```
    pub fn for_capability(&self, capability: &Capability) -> u64 {
        let Some(granted) = largest_grant::<u64>(capability, CAP_STREAM_SPILL) else {
            return self.ceiling;
        };
        granted.max(self.base).min(self.ceiling)
    }
}

/// The **byte pool** of one resolution tree (ledger #2238): how many more bytes the
/// kernel may BUFFER anywhere below the door that minted it, collecting a streamed body
/// ([`Body::Replay`](crate::Body::Replay), [`Body::Once`](crate::Body::Once)) into memory.
///
/// Shared and only narrowed, exactly as [`RetryBudget`] is: a sub-request whose capability
/// is entitled to less gets a child pool that also draws from the run's. A draw is
/// cumulative for the run, since a collected buffer leaves the kernel's sight once it is
/// handed over; a collect that FAILS (past the budget, or a stream error) gives back what
/// it drew, because it drops what it read.
///
/// What it counts: bytes the kernel reads out of a stream into memory. A buffer an
/// endpoint already built ([`Body::Bytes`](crate::Body::Bytes)) is handed over as it is,
/// drawing nothing, because it is already allocated and refusing it after the fact frees
/// nothing; and bytes streamed through without being held are not drawn either.
///
/// ```
/// use ikigai_core::IssueContext;
///
/// let run = IssueContext::new().with_memory_limit(10);
/// let pool = run.memory_budget().unwrap();
/// assert!(pool.try_draw(6));
/// assert!(!pool.try_draw(6), "4 left; a draw is all or nothing");
/// assert_eq!(pool.remaining(), 4);
/// ```
#[derive(Clone)]
pub struct MemoryBudget(Pool);

impl MemoryBudget {
    /// A fresh pool of `bytes`.
    pub(crate) fn new(bytes: u64) -> Self {
        MemoryBudget(Pool::new(bytes))
    }

    /// How many bytes may still be buffered: the least along this pool and every pool
    /// above it.
    pub fn remaining(&self) -> u64 {
        self.0.remaining()
    }

    /// Draw `bytes`: `true` if they were left (and are now drawn, here and in every pool
    /// above), `false` if not, in which case nothing is drawn. A buffering step calls this
    /// BEFORE it holds the bytes, and refuses with
    /// [`Error::OverBudget`](crate::Error::OverBudget) when it answers `false`.
    pub fn try_draw(&self, bytes: u64) -> bool {
        self.0.try_take(bytes)
    }

    /// Reserve `bytes` for as long as the returned [`Reservation`] lives (ledger #2381):
    /// they are drawn now, here and in every pool above, and given back when it drops. What
    /// a step holding a WINDOW of a stream takes (a line, a tail, a parser buffer, a sniff
    /// prefix): it reserves the window, holds it, and drops the reservation with it, so a
    /// long stream never drains the pool, and an early return or a dropped stream cannot
    /// leak what it held.
    ///
    /// Past what is left the answer is [`Error::OverBudget`](crate::Error::OverBudget),
    /// and nothing is drawn. Use [`try_draw`](Self::try_draw) instead for bytes that are
    /// handed over and leave the step's sight (a collected answer).
    ///
    /// ```
    /// use ikigai_core::{Error, IssueContext};
    ///
    /// let run = IssueContext::new().with_memory_limit(10);
    /// let pool = run.memory_budget().unwrap();
    /// for _line in 0..1_000 {
    ///     let held = pool.reserve(8).unwrap(); // one 8-byte line at a time
    ///     assert_eq!(pool.remaining(), 2);
    ///     drop(held);
    /// }
    /// assert_eq!(pool.remaining(), 10, "a thousand lines, one at a time, drain nothing");
    /// assert!(matches!(pool.reserve(11), Err(Error::OverBudget { .. })));
    /// assert_eq!(pool.remaining(), 10);
    /// ```
    pub fn reserve(&self, bytes: u64) -> crate::Result<Reservation> {
        if self.0.try_take(bytes) {
            Ok(Reservation {
                pool: self.0.clone(),
                bytes,
            })
        } else {
            Err(over_reservation(bytes, self.remaining()))
        }
    }

    /// Give back `bytes` this pool drew, when what they were drawn for was dropped (a
    /// failed collect). Crate-private: outside the crate a [`Reservation`] gives bytes back,
    /// so nothing can give back more than it drew.
    pub(crate) fn give_back(&self, bytes: u64) {
        self.0.give_back(bytes)
    }

    /// A view of this pool limited to `limit` bytes.
    pub(crate) fn narrowed(&self, limit: u64) -> MemoryBudget {
        MemoryBudget(self.0.narrowed(limit))
    }
}

fn over_reservation(wanted: u64, left: u64) -> crate::Error {
    crate::Error::OverBudget {
        detail: format!(
            "a reservation of {wanted} byte(s) is larger than the run's byte budget: {left} \
             byte(s) left (a `urn:cap:stream:memory:<bytes>` grant raises it, up to the host's \
             ceiling)"
        ),
    }
}

/// Bytes held from a [`MemoryBudget`] for as long as this lives
/// ([`MemoryBudget::reserve`]): given back to the pool, and every pool above it, when it
/// drops, however the step holding it ends.
///
/// A window that changes size moves with [`try_grow`](Self::try_grow) and
/// [`shrink`](Self::shrink); a step that hands its buffer over for good (so the bytes leave
/// its sight and stay held) ends the reservation with [`keep`](Self::keep).
///
/// ```
/// use ikigai_core::{Error, IssueContext};
///
/// let run = IssueContext::new().with_memory_limit(10);
/// let pool = run.memory_budget().unwrap();
/// let mut window = pool.reserve(4).unwrap();
/// window.try_grow(5).unwrap();
/// assert!(matches!(window.try_grow(2), Err(Error::OverBudget { .. })));
/// assert_eq!(window.bytes(), 9);
/// window.shrink(6);
/// assert_eq!((window.bytes(), pool.remaining()), (3, 7));
/// drop(window);
/// assert_eq!(pool.remaining(), 10);
/// pool.reserve(4).unwrap().keep();
/// assert_eq!(pool.remaining(), 6, "kept: drawn for the run, as try_draw is");
/// ```
#[must_use = "a reservation gives its bytes back when it drops"]
pub struct Reservation {
    pool: Pool,
    bytes: u64,
}

impl Reservation {
    /// How many bytes this holds.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Hold `more` bytes besides: all or nothing, [`Error::OverBudget`](crate::Error::OverBudget)
    /// past what the pool has left, in which case this still holds what it held.
    pub fn try_grow(&mut self, more: u64) -> crate::Result<()> {
        if self.pool.try_take(more) {
            self.bytes += more;
            Ok(())
        } else {
            Err(over_reservation(more, self.pool.remaining()))
        }
    }

    /// Give back `fewer` bytes now (at most what this holds), keeping the rest.
    pub fn shrink(&mut self, fewer: u64) {
        let fewer = fewer.min(self.bytes);
        self.pool.give_back(fewer);
        self.bytes -= fewer;
    }

    /// End the reservation WITHOUT giving its bytes back: they stay drawn for the run, as
    /// a [`MemoryBudget::try_draw`] is, because the buffer they cover was handed over.
    pub fn keep(mut self) {
        self.bytes = 0;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.pool.give_back(self.bytes);
        }
    }
}

impl fmt::Debug for Reservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reservation")
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl fmt::Debug for MemoryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryBudget")
            .field("remaining", &self.remaining())
            .finish()
    }
}

impl fmt::Debug for RetryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryBudget")
            .field("remaining", &self.remaining())
            .finish()
    }
}

/// What a run carries from the door that issued it down every sub-request, beyond the
/// request and its capability (ledger #1546): its retry pool, its deadline, and its byte
/// pool (ledger #2238).
///
/// A door mints one per run with [`new`](Self::new) and the narrowing builders, and issues
/// under it with [`Kernel::issue_with_context`](crate::Kernel::issue_with_context). Both
/// halves only ever NARROW: [`with_not_after`](Self::with_not_after) keeps the earlier
/// deadline, [`with_retry_limit`](Self::with_retry_limit) and
/// [`with_memory_limit`](Self::with_memory_limit) the smaller count, and the kernel
/// narrows each count again to what each request's capability is entitled to. So a peer that
/// carries a context across a wire can shorten its own budget, never lengthen it.
///
/// `#[non_exhaustive]` and opaque: later halves (a fuel budget, ledger #1889) are added here
/// without a new seam.
///
/// ```
/// use ikigai_core::{IssueContext, Time};
///
/// let run = IssueContext::new()
///     .with_not_after(Time::from_millis(5_000))
///     .with_not_after(Time::from_millis(9_000)); // a later deadline does not extend it
/// assert_eq!(run.not_after(), Some(Time::from_millis(5_000)));
/// assert!(run.retry_budget().is_none(), "no pool until a door or the kernel mints one");
/// ```
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct IssueContext {
    retries: Option<RetryBudget>,
    not_after: Option<Time>,
    memory: Option<MemoryBudget>,
    /// A body handed to ONE request by content id (ledger #2239): what a plan attaches
    /// for an incremental step, resolved only by an invocation whose request carries
    /// `ArgRef::Content` of that id. In-process: it never crosses a wire.
    attached: Option<Arc<Attached>>,
}

/// A body attached to a run by content id ([`IssueContext::attached`]).
pub(crate) struct Attached {
    id: crate::content::ContentId,
    /// The body; taken by the first reader when it is one-shot.
    body: std::sync::Mutex<Option<crate::Body>>,
    /// Retained after a one-shot body is taken; independent of content identity.
    reopenable: bool,
    /// Whether `id` is a nonce rather than the digest of the bytes (a body whose digest is
    /// not known before it is read): a request carrying it can never repeat, so nothing
    /// built on it is worth storing.
    nonce: bool,
}

impl Attached {
    pub(crate) fn is_reopenable(&self) -> bool {
        self.reopenable
    }

    /// The attached body for `id`, when it is this one: a second handle on a body that can
    /// be read again, or a one-shot body taken by its first reader (a second reader gets
    /// `Some(None)`).
    pub(crate) fn body_for(&self, id: &crate::content::ContentId) -> Option<Option<crate::Body>> {
        if self.id != *id {
            return None;
        }
        let mut slot = self.body.lock().expect("attached body lock");
        Some(match slot.as_ref().and_then(crate::Body::share) {
            Some(shared) => Some(shared),
            None => slot.take(),
        })
    }

    /// Whether the attachment's id is a nonce ([`Attached::nonce`]).
    pub(crate) fn is_nonce(&self) -> bool {
        self.nonce
    }

    /// The content id the attachment answers to.
    pub(crate) fn id(&self) -> &crate::content::ContentId {
        &self.id
    }
}

impl fmt::Debug for Attached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attached")
            .field("id", &self.id)
            .field("nonce", &self.nonce)
            .finish()
    }
}

impl IssueContext {
    /// An empty context: no pool and no deadline. What every request ran under before
    /// 0.1.108, and what [`Kernel::issue`](crate::Kernel::issue) still uses; the kernel
    /// mints the byte pool from the capability (ledger #2238).
    pub fn new() -> Self {
        IssueContext::default()
    }

    /// Bound the run by `deadline` on the kernel clock (builder). Keeps the earlier of this
    /// and any deadline already set.
    pub fn with_not_after(mut self, deadline: Time) -> Self {
        self.not_after = Some(self.not_after.map_or(deadline, |d| d.min(deadline)));
        self
    }

    /// Allow the run at most `retries` re-issues in all (builder): a pool of `retries`, or the
    /// existing pool narrowed to it. The kernel narrows it again to what the capability is
    /// entitled to, and drops it on a kernel with no retry budget configured (decision 5:
    /// there is no ceiling to narrow it under, so the overlays keep their own `attempts`).
    pub fn with_retry_limit(mut self, retries: u32) -> Self {
        self.retries = Some(match self.retries {
            Some(pool) => pool.narrowed(retries),
            None => RetryBudget::new(retries),
        });
        self
    }

    /// The run's retry pool, `None` when nothing minted one.
    pub fn retry_budget(&self) -> Option<&RetryBudget> {
        self.retries.as_ref()
    }

    /// Allow the run to buffer at most `bytes` in all (builder): a pool of `bytes`, or the
    /// existing pool narrowed to it. The kernel narrows it again to what each request's
    /// capability is entitled to ([`MemoryLimits::for_capability`]), so a door can lower a
    /// run's byte budget and never raise it.
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory = Some(match self.memory {
            Some(pool) => pool.narrowed(bytes),
            None => MemoryBudget::new(bytes),
        });
        self
    }

    /// The run's byte pool, `None` until a door or the kernel mints one. Every request
    /// the kernel runs has one: the kernel mints it from the capability on admission.
    pub fn memory_budget(&self) -> Option<&MemoryBudget> {
        self.memory.as_ref()
    }

    /// The run's deadline on the kernel clock, `None` when the door set none.
    pub fn not_after(&self) -> Option<Time> {
        self.not_after
    }

    /// The context a request under `capability` runs with on a kernel whose retry policy is
    /// `limits` and whose byte policy is `memory`: with no retry policy, no retry pool
    /// (today's behavior); with one, the pool narrowed to the capability's entitlement, or
    /// minted at it when there is none yet. The byte pool is always admitted the same way,
    /// since there is always a byte policy. The deadline is carried as it is. Admitting an
    /// admitted context again changes nothing (each pool is already within its
    /// entitlement, so narrowing hands back the same pool).
    pub(crate) fn admitted(
        self,
        limits: Option<&RetryLimits>,
        memory: &MemoryLimits,
        capability: &Capability,
    ) -> Self {
        let retries = limits.map(|limits| {
            let entitled = limits.for_capability(capability);
            match self.retries {
                Some(pool) => pool.narrowed(entitled),
                None => RetryBudget::new(entitled),
            }
        });
        let entitled = memory.for_capability(capability);
        let memory = match self.memory {
            Some(pool) => pool.narrowed(entitled),
            None => MemoryBudget::new(entitled),
        };
        IssueContext {
            retries,
            not_after: self.not_after,
            memory: Some(memory),
            attached: self.attached,
        }
    }

    /// This context with `body` attached under `id` (ledger #2239): what
    /// [`Invocation::transrept`](crate::Invocation::transrept) hands an incremental step.
    /// `nonce` says `id` is not the bytes' digest.
    pub(crate) fn with_attached(
        mut self,
        id: crate::content::ContentId,
        body: crate::Body,
        nonce: bool,
    ) -> Self {
        self.attached = Some(Arc::new(Attached {
            id,
            reopenable: !body.is_one_shot(),
            body: std::sync::Mutex::new(Some(body)),
            nonce,
        }));
        self
    }

    /// The attached body, if any ([`with_attached`](Self::with_attached)).
    pub(crate) fn attached(&self) -> Option<&Attached> {
        self.attached.as_deref()
    }

    /// This context with no body attached: what the kernel runs a request under once it
    /// has collected the attached body for an argument that does not read one (ledger
    /// #2402), so nothing below it can reach the body again.
    pub(crate) fn without_attached(mut self) -> Self {
        self.attached = None;
        self
    }

    /// Whether this context's byte pool is `other`'s very pool (not merely an equal count).
    #[cfg(test)]
    pub(crate) fn shares_memory_with(&self, other: &IssueContext) -> bool {
        match (&self.memory, &other.memory) {
            (Some(a), Some(b)) => a.0.same(&b.0),
            _ => false,
        }
    }
}

/// How a host issues one request (ledger #2402): the resolution chain it resolves in, the
/// provenance of the upstream stage whose output it carries, the run it belongs to, a
/// tracer, and a **live body attached** as the value of one named argument. What
/// [`Kernel::issue_body_with`](crate::Kernel::issue_body_with) and
/// [`Kernel::issue_with`](crate::Kernel::issue_with) take; each part defaults to what
/// [`Kernel::issue`](crate::Kernel::issue) uses (the empty chain, no upstream, a fresh run,
/// the kernel's global tracer, nothing attached), so a builder names only what differs.
///
/// **The attached body** ([`attach`](Self::attach)) is how an engine hands a stage the
/// upstream's body without collecting it between the two, and how a door hands an endpoint
/// an upload in flight. The request carries the argument as an
/// [`ArgRef::Content`](crate::ArgRef::Content) id, and the body rides this issue, in process only: it never
/// crosses a wire. The id is the body's digest when it knows one (a spilled body, a
/// [`Replay`](crate::Replay) with a validator), so the answer is cached on it like any
/// value; otherwise it is a nonce, and the answer is never stored (the uncached note says
/// `one-shot content`). A one-shot body is read once: a retry of the same request finds it
/// spent and answers `Conflict`. Spill it first (`urn:stream:spill`) when the stage must be
/// retried or cached.
///
/// ★ **Only an argument declared [`reads_body`](crate::ArgSpec::reads_body) receives the
/// body as it is** (or an incremental transreptor's `content`, the promise core made first,
/// ledger #2239). For any other argument the KERNEL collects the body under the run's byte
/// budget before the endpoint is entered, and the endpoint sees the inline bytes a by-value
/// pipe would have handed it, one request and one cache entry with it. Past the budget the
/// answer is `OverBudget` naming the argument, never a shorter value. The trace says which
/// happened: a collected argument is named under [`COLLECTED_NOTE`](crate::COLLECTED_NOTE).
/// The kernel REFUSES (`InvalidArgument`) a body attached to a `Meta` request or to a kernel
/// operation (`urn:kernel:*`), neither of which reads an argument as a body, and one attached
/// under a name the request already carries.
///
/// `#[non_exhaustive]`: later parts are added here without a new entry point.
///
/// ```
/// use std::sync::Arc;
/// use futures::executor::block_on;
/// use ikigai_core::{
///     ArgSpec, AsyncFnEndpoint, Body, BodyIssue, Bytes, Capability, Description, EndpointSpace,
///     Exact, Iri, Kernel, ReprType, Representation, Request, Verb,
/// };
///
/// // A stage that reads `content` as a body, and says so.
/// let count = AsyncFnEndpoint::new("count", |inv| {
///     Box::pin(async move {
///         let body = inv.content_body("content").await?;
///         let n = inv.collect(body).await?.len();
///         Ok(Representation::new(ReprType::new("text/plain"), n.to_string().into_bytes()))
///     })
/// })
/// .with_description(
///     Description::new("count")
///         .verb(Verb::Source)
///         .input(ArgSpec::new("content").reads_body()),
/// );
/// let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new("urn:count"), count)));
/// let upload = Body::once(futures::stream::iter([
///     Ok(Bytes::from_static(b"abc")),
///     Ok(Bytes::from_static(b"de")),
/// ]));
/// let read = Request::new(Verb::Source, Iri::parse("urn:count").unwrap());
/// let issue = BodyIssue::new().attach("content", upload);
/// let answer = block_on(kernel.issue_with(read, &Capability::root(), issue)).unwrap();
/// assert_eq!(answer.bytes, b"5");
/// ```
#[derive(Default)]
#[non_exhaustive]
pub struct BodyIssue {
    pub(crate) scope: crate::space::Scope,
    pub(crate) incoming: Option<crate::repr::Provenance>,
    pub(crate) context: IssueContext,
    pub(crate) tracer: Option<Arc<dyn crate::kernel::Tracer>>,
    pub(crate) attach: Option<(String, crate::Body)>,
}

impl BodyIssue {
    /// An issue like [`Kernel::issue`](crate::Kernel::issue)'s: the empty chain, no
    /// upstream, a fresh run, the kernel's global tracer, nothing attached.
    pub fn new() -> Self {
        BodyIssue::default()
    }

    /// Resolve in `scope` (builder), as [`Kernel::issue_in`](crate::Kernel::issue_in) does.
    pub fn in_scope(mut self, scope: crate::space::Scope) -> Self {
        self.scope = scope;
        self
    }

    /// Fold an upstream stage's provenance into the answer (builder), as
    /// [`Kernel::issue_with_incoming`](crate::Kernel::issue_with_incoming) does: the answer
    /// is no more cacheable than the upstream, and hangs from its threads.
    pub fn with_incoming(mut self, incoming: crate::repr::Provenance) -> Self {
        self.incoming = Some(incoming);
        self
    }

    /// Run as part of `context` (builder), as
    /// [`Kernel::issue_with_context`](crate::Kernel::issue_with_context) does.
    pub fn with_context(mut self, context: IssueContext) -> Self {
        self.context = context;
        self
    }

    /// Record the resolution into `tracer` (builder), as
    /// [`Kernel::issue_traced`](crate::Kernel::issue_traced) does.
    pub fn traced(mut self, tracer: Arc<dyn crate::kernel::Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attach `body` as the value of argument `name` (builder): see the type's
    /// documentation. A second attach replaces the first: one issue carries one body.
    pub fn attach(mut self, name: impl Into<String>, body: crate::Body) -> Self {
        self.attach = Some((name.into(), body));
        self
    }

    /// The resolution chain supplied by [`Self::in_scope`].
    pub fn scope(&self) -> &crate::space::Scope {
        &self.scope
    }

    /// The upstream provenance, if supplied by [`Self::with_incoming`].
    pub fn incoming(&self) -> Option<&crate::repr::Provenance> {
        self.incoming.as_ref()
    }

    /// The run context, including any shared memory and retry budgets.
    pub fn context(&self) -> &IssueContext {
        &self.context
    }

    /// The per-issue tracer, or `None` to use the kernel's global tracer.
    pub fn tracer(&self) -> Option<&Arc<dyn crate::kernel::Tracer>> {
        self.tracer.as_ref()
    }

    /// The attached argument name and body, without consuming either.
    pub fn attachment(&self) -> Option<(&str, &crate::Body)> {
        self.attach
            .as_ref()
            .map(|(name, body)| (name.as_str(), body))
    }

    /// Take the attached argument name and body, leaving no attachment.
    ///
    /// A resolver default can collect the body through [`Self::context`]'s memory
    /// budget and forward the remaining issue with the collected argument. The
    /// scope, provenance, run context and tracer are retained. A second take
    /// returns `None`; a one-shot body is moved, never cloned or read here.
    pub fn take_attachment(&mut self) -> Option<(String, crate::Body)> {
        self.attach.take()
    }
}

impl fmt::Debug for BodyIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyIssue")
            .field("scope", &self.scope.to_string())
            .field("incoming", &self.incoming)
            .field("context", &self.context)
            .field("traced", &self.tracer.is_some())
            .field("attach", &self.attach.as_ref().map(|(name, _)| name))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_pool_spends_from_its_parent_and_refunds_on_failure() {
        let parent = RetryBudget::new(2);
        let child = parent.narrowed(1);
        assert_eq!(child.remaining(), 1);
        assert!(child.try_spend());
        assert_eq!(parent.remaining(), 1, "a spend below is a spend above");
        assert!(!child.try_spend(), "the child's own limit holds");
        assert_eq!(parent.remaining(), 1, "a refused spend takes nothing");
        // Exhaust the parent; a fresh child of it has nothing to give.
        assert!(parent.try_spend());
        let late = parent.narrowed(5);
        assert_eq!(late.remaining(), 0);
        assert!(!late.try_spend());
    }

    #[test]
    fn narrowing_to_no_less_than_what_is_left_shares_the_pool() {
        let pool = RetryBudget::new(2);
        let same = pool.narrowed(7);
        assert!(same.try_spend());
        assert_eq!(pool.remaining(), 1);
        assert!(pool.0.same(&same.0));
    }

    #[test]
    fn a_base_above_the_ceiling_is_lowered_to_it() {
        let limits = RetryLimits::new(9, 2);
        assert_eq!(limits.base(), 2);
        assert_eq!(limits.for_capability(&Capability::scoped(["urn:x"])), 2);
    }

    #[test]
    fn a_denied_or_malformed_grant_counts_for_nothing() {
        let limits = RetryLimits::new(0, 5);
        for scopes in [
            vec!["urn:cap:retry:x".to_string()],
            vec!["urn:cap:retry:-1".to_string()],
            vec!["urn:cap:retry:".to_string()],
        ] {
            assert_eq!(
                limits.for_capability(&Capability::scoped(scopes.clone())),
                0,
                "{scopes:?}"
            );
        }
    }

    #[test]
    fn admission_with_no_policy_drops_the_pool_and_keeps_the_deadline() {
        let run = IssueContext::new()
            .with_retry_limit(3)
            .with_not_after(Time::from_millis(10));
        let admitted = run.admitted(None, &MemoryLimits::default(), &Capability::root());
        assert!(admitted.retry_budget().is_none());
        assert_eq!(admitted.not_after(), Some(Time::from_millis(10)));
    }

    #[test]
    fn admission_narrows_a_door_pool_to_the_entitlement() {
        let limits = RetryLimits::new(0, 4);
        let door = IssueContext::new().with_retry_limit(3);
        let one = Capability::scoped([cap_retry(1)]);
        let admitted = door
            .clone()
            .admitted(Some(&limits), &MemoryLimits::default(), &one);
        assert_eq!(admitted.retry_budget().unwrap().remaining(), 1);
        // Root is entitled to the ceiling, and the door's 3 is the tighter.
        let rooted = door.admitted(Some(&limits), &MemoryLimits::default(), &Capability::root());
        assert_eq!(rooted.retry_budget().unwrap().remaining(), 3);
        // No door pool: minted at the entitlement.
        let minted = IssueContext::new().admitted(
            Some(&limits),
            &MemoryLimits::default(),
            &Capability::root(),
        );
        assert_eq!(minted.retry_budget().unwrap().remaining(), 4);
    }

    #[test]
    fn a_byte_draw_is_all_or_nothing_up_the_chain_and_can_be_given_back() {
        let run = MemoryBudget::new(100);
        let child = run.narrowed(30);
        assert!(child.try_draw(20));
        assert_eq!(run.remaining(), 80, "a draw below is a draw above");
        assert!(
            !child.try_draw(20),
            "the child's own 10 left does not cover 20"
        );
        assert_eq!(run.remaining(), 80, "a refused draw takes nothing anywhere");
        child.give_back(20);
        assert_eq!((child.remaining(), run.remaining()), (30, 100));
    }

    #[test]
    fn the_byte_pool_is_always_admitted_and_narrowed_to_the_entitlement() {
        let limits = MemoryLimits::new(1_000, 1_000_000);
        // No door pool: minted at the entitlement, retry policy or not.
        let root = IssueContext::new().admitted(None, &limits, &Capability::root());
        assert_eq!(root.memory_budget().unwrap().remaining(), 1_000_000);
        let plain = IssueContext::new().admitted(None, &limits, &Capability::scoped(["urn:x"]));
        assert_eq!(plain.memory_budget().unwrap().remaining(), 1_000);
        // A door's lower limit holds; a sub-request entitled to less draws from the run.
        let door = IssueContext::new().with_memory_limit(50_000);
        let admitted = door.admitted(None, &limits, &Capability::root());
        assert_eq!(admitted.memory_budget().unwrap().remaining(), 50_000);
        let narrow = admitted
            .clone()
            .admitted(None, &limits, &Capability::scoped(["urn:x"]));
        assert!(narrow.memory_budget().unwrap().try_draw(1_000));
        assert_eq!(admitted.memory_budget().unwrap().remaining(), 49_000);
        // Admitting again changes nothing: the same pool, not a copy.
        let again = admitted
            .clone()
            .admitted(None, &limits, &Capability::root());
        assert!(again.shares_memory_with(&admitted));
    }

    #[test]
    fn a_byte_grant_is_parsed_like_a_retry_grant() {
        let limits = MemoryLimits::new(0, u64::MAX);
        for (scopes, expected) in [
            (vec![cap_stream_memory(7)], 7),
            (vec![cap_stream_memory(7), cap_stream_memory(9)], 9),
            (vec!["urn:cap:stream:memory:lots".to_string()], 0),
            (vec!["urn:cap:stream:memory:-1".to_string()], 0),
            (vec![cap_retry(5)], 0),
        ] {
            assert_eq!(
                limits.for_capability(&Capability::scoped(scopes.clone())),
                expected,
                "{scopes:?}"
            );
        }
    }
}
