//! **Operational time** (ledger #1916; `docs/design/time.md`): the run's deadline is on the
//! kernel's operational clock, so how long it has left is measured on that clock, never on
//! a temporal corridor's pinned instant, and a kernel without a clock says so.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::executor::block_on;
use ikigai_core::{
    BodyIssue, Capability, Clock, EndpointSpace, Exact, FixedClock, FnEndpoint, Iri, IssueContext,
    Kernel, ReprType, Representation, Request, Scope, Time, Verb,
};

const NOW: u64 = 10_000;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// An endpoint answering `time_left` in ms, or `none`.
fn left_space() -> Arc<EndpointSpace> {
    let left = FnEndpoint::new("left", |inv| {
        let said = inv
            .time_left()
            .map_or("none".to_string(), |d| d.as_millis().to_string());
        Ok(Representation::new(
            ReprType::new("text/plain"),
            said.into_bytes(),
        ))
    });
    Arc::new(EndpointSpace::new().bind(Exact::new("urn:left"), left))
}

fn pinned(at: u64) -> Scope {
    Scope::empty().with_named_at(
        iri(&format!("urn:ctx:time:{at}")),
        Arc::new(EndpointSpace::new()),
        Arc::new(FixedClock::at(at)),
    )
}

fn left_in(kernel: &Kernel, scope: Scope, deadline: u64) -> String {
    let issue = BodyIssue::new()
        .in_scope(scope)
        .with_context(IssueContext::new().with_not_after(Time::from_millis(deadline)));
    let answer = block_on(kernel.issue_with(
        Request::new(Verb::Source, iri("urn:left")),
        &Capability::root(),
        issue,
    ))
    .unwrap();
    String::from_utf8(answer.bytes.to_vec()).unwrap()
}

/// A pinned PAST corridor must not lengthen the run's operational deadline.
#[test]
fn a_pinned_past_does_not_lengthen_the_time_left() {
    let kernel = Kernel::new(left_space()).with_clock(Arc::new(FixedClock::at(NOW)));
    assert_eq!(left_in(&kernel, Scope::empty(), NOW + 500), "500");
    assert_eq!(left_in(&kernel, pinned(1), NOW + 500), "500");
}

/// A pinned FUTURE corridor must not exhaust it.
#[test]
fn a_pinned_future_does_not_exhaust_the_time_left() {
    let kernel = Kernel::new(left_space()).with_clock(Arc::new(FixedClock::at(NOW)));
    assert_eq!(left_in(&kernel, pinned(NOW * 1_000), NOW + 500), "500");
}

/// The kernel's clock, moved by the test.
#[derive(Clone)]
struct Moving(Arc<AtomicU64>);
impl Clock for Moving {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

/// Absent: a kernel with no clock has no operational time, and a run's deadline there is
/// `None` from `time_left` but a refusal from `time_left_checked`, never unbounded.
#[test]
fn an_absent_clock_is_told_apart_from_no_deadline() {
    let checked = FnEndpoint::new("checked", |inv| {
        let said = match inv.time_left_checked() {
            Ok(None) => "unbounded".to_string(),
            Ok(Some(left)) => left.as_millis().to_string(),
            Err(error) => format!("refused: {error}"),
        };
        assert_eq!(inv.operational_now(), None, "no clock, no fallback");
        Ok(Representation::new(
            ReprType::new("text/plain"),
            said.into_bytes(),
        ))
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:checked"), checked),
    ));
    assert_eq!(kernel.now(), None);
    assert!(kernel.clock().is_none());
    let read = || Request::new(Verb::Source, iri("urn:checked"));
    let answer = block_on(kernel.issue(read(), &Capability::root())).unwrap();
    assert_eq!(answer.bytes, b"unbounded");
    let bounded = IssueContext::new().with_not_after(Time::from_millis(NOW));
    let answer = block_on(kernel.issue_with_context(read(), &Capability::root(), bounded)).unwrap();
    let said = String::from_utf8(answer.bytes.to_vec()).unwrap();
    assert!(said.starts_with("refused: unsupported: "), "{said}");
    assert!(said.contains("no clock"), "{said}");

    // And the plain reading still says `None` there, as it is documented to.
    assert_eq!(
        left_in(&Kernel::new(left_space()), Scope::empty(), NOW),
        "none"
    );
}

/// Fixed: the kernel's accessor and the invocation's agree with the clock installed.
#[test]
fn a_fixed_clock_is_the_kernels_operational_time() {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::at(NOW));
    let kernel = Kernel::new(left_space()).with_clock(Arc::clone(&clock));
    assert_eq!(kernel.now(), Some(Time::from_millis(NOW)));
    assert!(
        Arc::ptr_eq(kernel.clock().unwrap(), &clock),
        "the same source, shared"
    );
    assert_eq!(left_in(&kernel, Scope::empty(), NOW + 250), "250");
    assert_eq!(
        left_in(&kernel, Scope::empty(), NOW - 1),
        "0",
        "past is zero left"
    );
}

/// Moving: the time left shrinks as the kernel's clock moves, and `Kernel::now` follows.
#[test]
fn a_moving_clock_shrinks_the_time_left() {
    let moving = Moving(Arc::new(AtomicU64::new(NOW)));
    let kernel = Kernel::new(left_space()).with_clock(Arc::new(moving.clone()));
    assert_eq!(left_in(&kernel, Scope::empty(), NOW + 1_000), "1000");
    moving.0.store(NOW + 600, Ordering::SeqCst);
    assert_eq!(kernel.now(), Some(Time::from_millis(NOW + 600)));
    assert_eq!(left_in(&kernel, Scope::empty(), NOW + 1_000), "400");
    // Under a pinned corridor too: the corridor's instant is world time, not this.
    assert_eq!(left_in(&kernel, pinned(1), NOW + 1_000), "400");
}

/// Detached: the operational seam answers only where there is no issuer, independently
/// of the world-time override, and a world-time override does not move the time left.
#[test]
fn a_detached_invocation_reads_its_own_operational_clock() {
    use ikigai_core::{Bindings, Invocation};
    let request = Request::new(Verb::Source, iri("urn:x"));
    let bindings = Bindings::new();
    let cap = Capability::root();
    let deadline = Time::from_millis(NOW + 300);
    // A world clock alone does not measure a deadline.
    let world =
        Invocation::detached(&request, &bindings, &cap).with_clock(Arc::new(FixedClock::at(NOW)));
    let world_only = world.with_not_after(deadline);
    assert_eq!(world_only.now(), Some(Time::from_millis(NOW)));
    assert_eq!(world_only.operational_now(), None);
    assert_eq!(world_only.time_left(), None);
    assert!(world_only.time_left_checked().is_err());
    // The operational seam does, whatever the world clock says.
    let both = Invocation::detached(&request, &bindings, &cap)
        .with_clock(Arc::new(FixedClock::at(1)))
        .with_operational_clock(Arc::new(FixedClock::at(NOW)));
    let both = both.with_not_after(deadline);
    assert_eq!(both.now(), Some(Time::from_millis(1)));
    assert_eq!(both.time_left(), Some(Duration::from_millis(300)));
}

/// A world-time override an endpoint attaches inside a kernel run does not move the run's
/// time left, and neither does an operational override: the kernel's clock answers.
#[test]
fn an_override_inside_a_kernel_run_does_not_move_its_deadline() {
    let overriding = FnEndpoint::new("overriding", |inv| {
        let inv = inv
            .with_bindings(inv.bindings)
            .with_clock(Arc::new(FixedClock::at(1)))
            .with_operational_clock(Arc::new(FixedClock::at(1)));
        let said = inv.time_left().map_or(0, |d| d.as_millis()).to_string();
        Ok(Representation::new(
            ReprType::new("text/plain"),
            said.into_bytes(),
        ))
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:left"), overriding),
    ))
    .with_clock(Arc::new(FixedClock::at(NOW)));
    assert_eq!(left_in(&kernel, Scope::empty(), NOW + 500), "500");
}

/// What a wire door does with a peer's remaining time (ikigai-wire's `IssueWith`): the
/// deadline is rebuilt on the RECEIVING kernel's operational clock, so an endpoint there
/// sees the peer's time left, whatever epoch that clock is on (a replay clock decades
/// from the system's); a clockless receiver refuses the bounded request.
#[test]
fn a_peers_time_left_is_rebuilt_on_the_receiving_kernels_clock() {
    let replay = Kernel::new(left_space()).with_clock(Arc::new(FixedClock::at(NOW)));
    let peer_left = Duration::from_millis(1_500);
    let deadline = replay.deadline_in(peer_left).unwrap();
    assert_eq!(deadline, Time::from_millis(NOW + 1_500));
    assert_eq!(
        left_in(&replay, Scope::empty(), deadline.as_millis()),
        "1500"
    );
    // Clamped to a local deadline already set: never extended.
    let local = IssueContext::new()
        .with_not_after(Time::from_millis(NOW + 200))
        .with_not_after(deadline);
    assert_eq!(local.not_after(), Some(Time::from_millis(NOW + 200)));

    let clockless = Kernel::new(left_space());
    let refused = clockless.deadline_in(peer_left).unwrap_err();
    assert!(
        matches!(refused, ikigai_core::Error::Unsupported { verb: None, .. }),
        "{refused}"
    );
}
