//! **`Invocation::pinned_now`: the pinned instant on its own** (ledger #4285). `now()`
//! merges a stated clock, a temporal corridor's and the kernel's, and an endpoint never
//! holds its `Scope`, so before this it could not tell an as-of instant from a live one.
//! The SPARQL `NOW()` rewrite in `ikigai-store` needs exactly that: an instant it may
//! cache under (the corridor's, which the cache key covers) apart from the wall clock
//! (which it must not). These pin the reader: no corridor answers `None` whatever clock
//! the kernel has, a corridor answers its instant, and nested corridors the innermost.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    Capability, Clock, EndpointSpace, Exact, FixedClock, FnEndpoint, Invocation, Iri, Kernel,
    ReprType, Representation, Request, Scope, Time, Verb,
};

/// The kernel's own clock: "live" for these tests.
const LIVE: u64 = 2_000_000;
/// The instant the outer corridor pins.
const PINNED: u64 = 1_000_000;
/// The instant an inner corridor pins.
const LATER: u64 = 3_000_000;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn millis(time: Option<Time>) -> String {
    time.map(|t| t.as_millis().to_string())
        .unwrap_or_else(|| "none".to_string())
}

/// Answers what it reads: the pinned instant, then the merged time. Uncacheable, so
/// every issue below runs it.
fn reader() -> FnEndpoint {
    FnEndpoint::new("reader", |inv: &Invocation<'_>| {
        let seen = format!(
            "pinned={} now={}",
            millis(inv.pinned_now()),
            millis(inv.now())
        );
        Ok(Representation::new(
            ReprType::new("text/plain"),
            seen.into_bytes(),
        ))
    })
}

fn kernel() -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:reader"), reader()),
    ))
    .with_clock(Arc::new(FixedClock::at(LIVE)))
}

fn read(kernel: &Kernel, scope: Scope) -> String {
    let answer = block_on(kernel.issue_in(
        Request::new(Verb::Source, iri("urn:reader")),
        &Capability::root(),
        scope,
    ))
    .unwrap();
    String::from_utf8(answer.bytes.to_vec()).unwrap()
}

fn at(name: &str, millis: u64) -> Scope {
    Scope::empty().with_named_at(
        iri(name),
        Arc::new(EndpointSpace::new()),
        Arc::new(FixedClock::at(millis)),
    )
}

#[test]
fn with_no_corridor_there_is_no_pinned_instant() {
    let kernel = kernel();
    // The kernel's clock is live time, not a pinned instant.
    assert_eq!(
        read(&kernel, Scope::empty()),
        format!("pinned=none now={LIVE}")
    );
    // Nor is a corridor that pins no time.
    let untimed = Scope::empty().with_named(iri("urn:ctx:board"), Arc::new(EndpointSpace::new()));
    assert_eq!(read(&kernel, untimed), format!("pinned=none now={LIVE}"));
}

#[test]
fn a_corridor_pins_its_instant() {
    assert_eq!(
        read(&kernel(), at("urn:ctx:time:then", PINNED)),
        format!("pinned={PINNED} now={PINNED}")
    );
}

#[test]
fn nested_corridors_pin_the_innermost_instant() {
    let kernel = kernel();
    let nested = at("urn:ctx:time:then", PINNED).with_named_at(
        iri("urn:ctx:time:later"),
        Arc::new(EndpointSpace::new()),
        Arc::new(FixedClock::at(LATER)),
    );
    assert_eq!(read(&kernel, nested), format!("pinned={LATER} now={LATER}"));

    // Stacked chains nest the same way: `inner`'s corridor is innermost.
    let stacked = at("urn:ctx:time:then", PINNED).stack(&at("urn:ctx:time:later", LATER));
    assert_eq!(
        read(&kernel, stacked),
        format!("pinned={LATER} now={LATER}")
    );

    // A corridor that pins no time, injected inside, leaves the instant as it was.
    let inside = at("urn:ctx:time:then", PINNED)
        .with_named(iri("urn:ctx:board"), Arc::new(EndpointSpace::new()));
    assert_eq!(
        read(&kernel, inside),
        format!("pinned={PINNED} now={PINNED}")
    );
}

/// A clock that moves, for the one case `pinned_now` and `now` part under a corridor.
struct Moving(Arc<AtomicU64>);
impl Clock for Moving {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[test]
fn the_pinned_instant_is_the_one_read_at_injection() {
    // The instant the fingerprint hashes, so the cache key covers it; `now` reads the
    // corridor's clock live and may have moved since.
    let tick = Arc::new(AtomicU64::new(PINNED));
    let scope = Scope::empty().with_named_at(
        iri("urn:ctx:time:moving"),
        Arc::new(EndpointSpace::new()),
        Arc::new(Moving(Arc::clone(&tick))),
    );
    tick.store(PINNED + 5, Ordering::SeqCst);
    assert_eq!(
        read(&kernel(), scope),
        format!("pinned={PINNED} now={}", PINNED + 5)
    );
}
