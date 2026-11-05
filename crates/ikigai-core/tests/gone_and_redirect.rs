//! **A name can resolve to "elsewhere" or "no longer"** (ledger #3150), end to end through the
//! kernel: `Error::Gone` fails like every other error, and a redirect answer is an ordinary
//! answer, cached, threaded and handed to a composite as it is. Core follows nothing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, AsyncFnEndpoint, Capability, Description, EndpointSpace, Error, Exact, Expiry,
    FixedClock, FnEndpoint, Iri, Kernel, Redirect, RedirectStatus, ReprType, Representation,
    Request, Time, TraceEvent, Tracer, Verb, FAILED_NOTE,
};

const NAME: &str = "urn:t:name";
const TARGET: &str = "urn:t:target";
const COMPOSITE: &str = "urn:t:composite";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn see_other() -> Redirect {
    Redirect::new(RedirectStatus::SeeOther, TARGET).unwrap()
}

/// An endpoint at `NAME` and the kernel around it, counting its runs.
fn counted(
    answer: impl Fn(&ikigai_core::Invocation<'_>) -> ikigai_core::Result<Representation>
        + Send
        + Sync
        + 'static,
) -> (Kernel, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    let endpoint = FnEndpoint::new("name", move |inv| {
        counter.fetch_add(1, Ordering::SeqCst);
        answer(inv)
    });
    let space = EndpointSpace::new().bind(Exact::new(NAME), endpoint);
    let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(FixedClock::at(10)));
    (kernel, runs)
}

/// `NAME` answering what `answer` says, `TARGET` counting its runs, and `COMPOSITE` handing
/// `compose` what it read from `NAME`.
fn composed(
    answer: impl Fn() -> ikigai_core::Result<Representation> + Send + Sync + 'static,
    compose: impl Fn(ikigai_core::Result<Representation>) -> Option<ikigai_core::Result<Representation>>
        + Send
        + Sync
        + Clone
        + 'static,
) -> (Kernel, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let target_runs = Arc::new(AtomicUsize::new(0));
    let composite_runs = Arc::new(AtomicUsize::new(0));
    let target_counter = Arc::clone(&target_runs);
    let composite_counter = Arc::clone(&composite_runs);
    let space = EndpointSpace::new()
        .bind(Exact::new(NAME), FnEndpoint::new("name", move |_| answer()))
        .bind(
            Exact::new(TARGET),
            FnEndpoint::new("target", move |_| {
                target_counter.fetch_add(1, Ordering::SeqCst);
                Ok(text("the thing itself"))
            }),
        )
        .bind(
            Exact::new(COMPOSITE),
            AsyncFnEndpoint::new("composite", move |inv| {
                composite_counter.fetch_add(1, Ordering::SeqCst);
                let compose = compose.clone();
                Box::pin(async move {
                    let read = inv.issue(source(NAME)).await;
                    match compose(read) {
                        Some(answer) => answer,
                        // The caller asked to follow, which it does itself.
                        None => inv.issue(source(TARGET)).await,
                    }
                })
            }),
        );
    let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(FixedClock::at(10)));
    (kernel, target_runs, composite_runs)
}

fn read(kernel: &Kernel, target: &str) -> ikigai_core::Result<Representation> {
    block_on(kernel.issue(source(target), &Capability::root()))
}

#[test]
fn a_redirect_is_an_ok_answer_that_reads_back_through_both_entry_points() {
    let (kernel, _) = counted(|_| Ok(Representation::redirect(see_other())));
    let answer = read(&kernel, NAME).unwrap();
    assert_eq!(answer.as_redirect(), Some(see_other()));
    assert_eq!(answer.bytes, br#"{"status":303,"location":"urn:t:target"}"#);
    let body = block_on(kernel.issue_body(source(NAME), &Capability::root())).unwrap();
    assert_eq!(body.as_redirect(), Some(see_other()));
}

#[test]
fn a_redirect_without_an_expiry_is_not_cached() {
    let (kernel, runs) = counted(|_| Ok(Representation::redirect(see_other())));
    read(&kernel, NAME).unwrap();
    read(&kernel, NAME).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

/// The reason it is a media type: an edit to the registry behind a redirect cuts it, and the
/// next read asks the registry again.
#[test]
fn a_cacheable_redirect_hangs_from_its_thread_and_a_cut_drops_it() {
    let registry = Arc::new(Mutex::new(TARGET.to_string()));
    let current = Arc::clone(&registry);
    let (kernel, runs) = counted(move |_| {
        let location = current.lock().unwrap().clone();
        let redirect = Redirect::new(RedirectStatus::Found, location).unwrap();
        Ok(Representation::redirect(redirect)
            .cacheable()
            .depends_on("urn:t:registry"))
    });
    let first = read(&kernel, NAME).unwrap().as_redirect().unwrap();
    assert_eq!(first.location.as_str(), TARGET);
    read(&kernel, NAME).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the second read was a hit");

    *registry.lock().unwrap() = "https://elsewhere.example/moved".to_string();
    let stale = read(&kernel, NAME).unwrap().as_redirect().unwrap();
    assert_eq!(stale.location.as_str(), TARGET, "no cut, no change");
    kernel.cut("urn:t:registry");
    let moved = read(&kernel, NAME).unwrap().as_redirect().unwrap();
    assert_eq!(moved.location.as_str(), "https://elsewhere.example/moved");
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn a_composite_receives_a_redirect_as_it_is_and_core_does_not_follow_it() {
    let (kernel, target_runs, _) = composed(|| Ok(Representation::redirect(see_other())), Some);
    let answer = read(&kernel, COMPOSITE).unwrap();
    assert_eq!(answer.as_redirect(), Some(see_other()));
    assert_eq!(target_runs.load(Ordering::SeqCst), 0, "nothing followed it");
    read(&kernel, NAME).unwrap();
    assert_eq!(
        target_runs.load(Ordering::SeqCst),
        0,
        "nor at the top level"
    );
}

#[test]
fn a_composite_that_wants_to_follow_a_redirect_does_so_itself() {
    let (kernel, target_runs, _) = composed(
        || Ok(Representation::redirect(see_other())),
        |read| match read {
            Ok(answer) if answer.as_redirect().is_some() => None,
            other => Some(other),
        },
    );
    assert_eq!(read(&kernel, COMPOSITE).unwrap().bytes, b"the thing itself");
    assert_eq!(target_runs.load(Ordering::SeqCst), 1);
}

/// Nothing special to core: a `Sink` that answers with the reserved type gets it handed back.
#[test]
fn a_sink_answer_of_the_reserved_type_is_handed_back_like_any_other() {
    let (kernel, _) = counted(|_| Ok(Representation::redirect(see_other())));
    let request = Request::new(Verb::Sink, iri(NAME));
    let answer = block_on(kernel.issue(request, &Capability::root())).unwrap();
    assert_eq!(answer.as_redirect(), Some(see_other()));
}

#[test]
fn gone_without_a_stated_expiry_is_never_cached() {
    let (kernel, runs) = counted(|_| Err(Error::Gone("retired".to_string())));
    for _ in 0..2 {
        assert_eq!(
            read(&kernel, NAME).unwrap_err(),
            Error::Gone("retired".to_string())
        );
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn gone_with_a_stated_expiry_is_cached_and_a_cut_of_its_target_drops_it() {
    let (kernel, runs) = counted(|inv| {
        inv.failure_expires_at(Time::from_millis(500));
        Err(Error::Gone("retired".to_string()))
    });
    for _ in 0..2 {
        assert_eq!(
            read(&kernel, NAME).unwrap_err(),
            Error::Gone("retired".to_string()),
            "a cached failure keeps its variant"
        );
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the second read was served");
    kernel.cut(NAME);
    read(&kernel, NAME).unwrap_err();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

/// `Gone` hangs no thread, unlike `NotFound`: nothing could say what would clear it, so a
/// cacheable fallback over it is not kept.
#[test]
fn a_cacheable_fallback_over_gone_is_forced_uncacheable() {
    let (kernel, _, composite_runs) = composed(
        || Err(Error::Gone("retired".to_string())),
        |read| match read {
            Err(Error::Gone(_)) => Some(Ok(text("a tombstone page").cacheable())),
            other => Some(other),
        },
    );
    let answer = read(&kernel, COMPOSITE).unwrap();
    assert_eq!(answer.expiry, Expiry::Always);
    read(&kernel, COMPOSITE).unwrap();
    assert_eq!(composite_runs.load(Ordering::SeqCst), 2);
}

/// A reference argument whose referent was retired stays `Gone`, and says which argument
/// brought it here, as `NotFound` does.
#[test]
fn a_retired_referent_is_gone_naming_the_argument_that_refers_to_it() {
    let space = EndpointSpace::new()
        .bind(
            Exact::new(NAME),
            FnEndpoint::new("name", |_| Err(Error::Gone("retired".to_string()))),
        )
        .bind(
            Exact::new(COMPOSITE),
            FnEndpoint::new("reader", |inv| Ok(text(inv.inline_str("in")?))).with_description(
                Description::new("reader")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("in")),
            ),
        );
    let kernel = Kernel::new(Arc::new(space));
    let request = source(COMPOSITE).with_arg("in", ArgRef::Reference(iri(NAME)));
    let err = block_on(kernel.issue(request, &Capability::root())).unwrap_err();
    assert_eq!(
        err,
        Error::Gone("argument `in` refers to <urn:t:name>: retired".to_string())
    );
}

#[derive(Default)]
struct Recorder(Mutex<Vec<TraceEvent>>);

impl Tracer for Recorder {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// The trace reports the kind word and nothing from the message.
#[test]
fn a_gone_invocation_is_traced_as_gone() {
    let (kernel, _) = counted(|_| Err(Error::Gone("a secret path".to_string())));
    let recorder = Arc::new(Recorder::default());
    block_on(kernel.issue_traced(source(NAME), &Capability::root(), recorder.clone())).unwrap_err();
    let events = recorder.0.lock().unwrap();
    assert!(
        events
            .iter()
            .flat_map(|e| &e.notes)
            .any(|(k, v)| k == FAILED_NOTE && v == "gone"),
        "{events:?}"
    );
}
