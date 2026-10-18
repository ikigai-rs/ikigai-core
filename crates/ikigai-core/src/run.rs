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
//! ★ **Nothing changes for a kernel that configures nothing** (decision 5): without
//! [`Kernel::with_retry_budget`](crate::Kernel::with_retry_budget) there is no pool,
//! [`Invocation::retry_budget`](crate::Invocation::retry_budget) answers `None`, and every
//! retry overlay keeps its own `attempts`.

use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
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
        let Some(scopes) = capability.scopes() else {
            return self.ceiling;
        };
        let prefix = CAP_RETRY.trim_end_matches('*');
        let granted = scopes
            .iter()
            .filter(|scope| capability.allows(scope))
            .filter_map(|scope| scope.strip_prefix(prefix)?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        granted.max(self.base).min(self.ceiling)
    }
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
pub struct RetryBudget(Arc<Pool>);

struct Pool {
    left: AtomicU32,
    parent: Option<RetryBudget>,
}

impl RetryBudget {
    /// A fresh pool of `retries`.
    pub(crate) fn new(retries: u32) -> Self {
        RetryBudget(Arc::new(Pool {
            left: AtomicU32::new(retries),
            parent: None,
        }))
    }

    /// How many re-issues are left: the least along this pool and every pool above it.
    pub fn remaining(&self) -> u32 {
        let mut least = u32::MAX;
        let mut pool = Some(self);
        while let Some(p) = pool {
            least = least.min(p.0.left.load(Ordering::Acquire));
            pool = p.0.parent.as_ref();
        }
        least
    }

    /// Spend one re-issue: `true` if one was left (and is now spent, here and in every
    /// pool above), `false` if none was, in which case nothing is spent. An overlay calls
    /// this BEFORE it re-issues, and returns the error it received when it answers `false`.
    pub fn try_spend(&self) -> bool {
        let mut spent: Vec<&RetryBudget> = Vec::new();
        let mut pool = Some(self);
        while let Some(p) = pool {
            let took = take_one(&p.0.left);
            if !took {
                // Give back what this call took below the empty pool.
                for p in spent {
                    p.0.left.fetch_add(1, Ordering::AcqRel);
                }
                return false;
            }
            spent.push(p);
            pool = p.0.parent.as_ref();
        }
        true
    }

    /// A view of this pool limited to `limit`: the same pool when `limit` is no less than
    /// what is left, else a child that draws from it.
    pub(crate) fn narrowed(&self, limit: u32) -> RetryBudget {
        if limit >= self.remaining() {
            return self.clone();
        }
        RetryBudget(Arc::new(Pool {
            left: AtomicU32::new(limit),
            parent: Some(self.clone()),
        }))
    }
}

/// Decrement `left` if it is above zero: a compare-and-swap loop, so a spend that finds the
/// pool empty takes nothing.
fn take_one(left: &AtomicU32) -> bool {
    let mut current = left.load(Ordering::Acquire);
    while let Some(next) = current.checked_sub(1) {
        match left.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(seen) => current = seen,
        }
    }
    false
}

impl fmt::Debug for RetryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryBudget")
            .field("remaining", &self.remaining())
            .finish()
    }
}

/// What a run carries from the door that issued it down every sub-request, beyond the
/// request and its capability (ledger #1546): its retry pool and its deadline.
///
/// A door mints one per run with [`new`](Self::new) and the narrowing builders, and issues
/// under it with [`Kernel::issue_with_context`](crate::Kernel::issue_with_context). Both
/// halves only ever NARROW: [`with_not_after`](Self::with_not_after) keeps the earlier
/// deadline, [`with_retry_limit`](Self::with_retry_limit) the smaller count, and the kernel
/// narrows the count again to what each request's capability is entitled to. So a peer that
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
}

impl IssueContext {
    /// An empty context: no pool and no deadline. What every request ran under before
    /// 0.1.108, and what [`Kernel::issue`](crate::Kernel::issue) still uses.
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

    /// The run's deadline on the kernel clock, `None` when the door set none.
    pub fn not_after(&self) -> Option<Time> {
        self.not_after
    }

    /// The context a request under `capability` runs with on a kernel whose retry policy is
    /// `limits`: with no policy, no pool (today's behavior); with one, the pool narrowed to
    /// the capability's entitlement, or minted at it when there is none yet. The deadline is
    /// carried as it is.
    pub(crate) fn admitted(self, limits: Option<&RetryLimits>, capability: &Capability) -> Self {
        let retries = limits.map(|limits| {
            let entitled = limits.for_capability(capability);
            match self.retries {
                Some(pool) => pool.narrowed(entitled),
                None => RetryBudget::new(entitled),
            }
        });
        IssueContext {
            retries,
            not_after: self.not_after,
        }
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
        assert!(Arc::ptr_eq(&pool.0, &same.0));
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
        let admitted = run.admitted(None, &Capability::root());
        assert!(admitted.retry_budget().is_none());
        assert_eq!(admitted.not_after(), Some(Time::from_millis(10)));
    }

    #[test]
    fn admission_narrows_a_door_pool_to_the_entitlement() {
        let limits = RetryLimits::new(0, 4);
        let door = IssueContext::new().with_retry_limit(3);
        let one = Capability::scoped([cap_retry(1)]);
        let admitted = door.clone().admitted(Some(&limits), &one);
        assert_eq!(admitted.retry_budget().unwrap().remaining(), 1);
        // Root is entitled to the ceiling, and the door's 3 is the tighter.
        let rooted = door.admitted(Some(&limits), &Capability::root());
        assert_eq!(rooted.retry_budget().unwrap().remaining(), 3);
        // No door pool: minted at the entitlement.
        let minted = IssueContext::new().admitted(Some(&limits), &Capability::root());
        assert_eq!(minted.retry_budget().unwrap().remaining(), 4);
    }
}
