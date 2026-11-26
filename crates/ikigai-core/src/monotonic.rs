//! Elapsed time for local windows and cooldowns, separate from epoch time.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// An explicitly supplied source of elapsed time.
///
/// Cloning a clock shares its source. Each constructor call creates a new source,
/// even if two host readers report the same values. This clock is not a kernel
/// [`Clock`](crate::Clock) and cannot supply world time or epoch deadlines.
#[derive(Clone)]
pub struct MonotonicClock {
    source: Arc<()>,
    read: Arc<dyn Fn() -> Duration + Send + Sync>,
}

impl MonotonicClock {
    /// Adapt a host reader that returns time since a fixed local origin.
    ///
    /// The reader must be nondecreasing, including across concurrent reads, and
    /// must not be driven by wall time. Readings may repeat. The host must not
    /// reset its origin or wrap its counter while this source is in use.
    pub fn from_host(read: impl Fn() -> Duration + Send + Sync + 'static) -> Self {
        Self {
            source: Arc::new(()),
            read: Arc::new(read),
        }
    }

    /// Start a native elapsed clock using [`std::time::Instant`].
    ///
    /// This never samples wall time. On wasm, supply an explicit host reader
    /// through [`Self::from_host`] instead.
    #[cfg(not(target_family = "wasm"))]
    pub fn native() -> Self {
        let origin = std::time::Instant::now();
        Self::from_host(move || origin.elapsed())
    }

    /// Sample this source.
    pub fn now(&self) -> MonotonicInstant {
        MonotonicInstant {
            source: self.source.clone(),
            ticks: (self.read)(),
        }
    }
}

/// An opaque reading or computed deadline on one [`MonotonicClock`].
///
/// It has no epoch representation and is not serializable. Use checked methods
/// to compare two instants so mixing sources cannot silently order them.
#[derive(Clone)]
pub struct MonotonicInstant {
    source: Arc<()>,
    ticks: Duration,
}

impl fmt::Debug for MonotonicInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MonotonicInstant").finish_non_exhaustive()
    }
}

impl MonotonicInstant {
    /// Add a duration on this source, or return an error if it cannot fit.
    pub fn checked_add(&self, duration: Duration) -> Result<Self, MonotonicError> {
        let ticks = self
            .ticks
            .checked_add(duration)
            .ok_or(MonotonicError::Overflow)?;
        Ok(Self {
            source: self.source.clone(),
            ticks,
        })
    }

    /// Compare two instants from the same source.
    pub fn checked_cmp(&self, other: &Self) -> Result<Ordering, MonotonicError> {
        self.check_source(other)?;
        Ok(self.ticks.cmp(&other.ticks))
    }

    /// Subtract an earlier instant on the same source.
    ///
    /// An instant after `self` returns [`MonotonicError::Reversed`].
    pub fn checked_duration_since(&self, earlier: &Self) -> Result<Duration, MonotonicError> {
        self.check_source(earlier)?;
        self.ticks
            .checked_sub(earlier.ticks)
            .ok_or(MonotonicError::Reversed)
    }

    /// Subtract an instant on the same source, returning zero for a future instant.
    ///
    /// Source mismatches still return an error. For example, use this to measure
    /// the duration remaining until a cooldown deadline:
    ///
    /// ```
    /// use ikigai_core::MonotonicClock;
    /// use std::time::Duration;
    ///
    /// let clock = MonotonicClock::from_host(|| Duration::from_secs(42));
    /// let deadline = clock.now().checked_add(Duration::from_secs(5))?;
    /// assert_eq!(deadline.saturating_duration_since(&clock.now())?, Duration::from_secs(5));
    /// # Ok::<(), ikigai_core::MonotonicError>(())
    /// ```
    pub fn saturating_duration_since(&self, earlier: &Self) -> Result<Duration, MonotonicError> {
        self.check_source(earlier)?;
        Ok(self.ticks.saturating_sub(earlier.ticks))
    }

    fn check_source(&self, other: &Self) -> Result<(), MonotonicError> {
        if Arc::ptr_eq(&self.source, &other.source) {
            Ok(())
        } else {
            Err(MonotonicError::DifferentSources)
        }
    }
}

/// An invalid operation on monotonic instants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MonotonicError {
    /// The instants came from different sources.
    DifferentSources,
    /// The duration would put the result outside the representable range.
    Overflow,
    /// The instant being subtracted is later than the starting instant.
    Reversed,
}

impl fmt::Display for MonotonicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DifferentSources => "monotonic instants come from different clock sources",
            Self::Overflow => "monotonic deadline is outside the representable range",
            Self::Reversed => "cannot measure elapsed time since a later monotonic instant",
        })
    }
}

impl std::error::Error for MonotonicError {}
