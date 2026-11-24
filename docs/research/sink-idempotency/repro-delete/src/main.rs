//! Reproduces what `ikigai-throttle`'s `Retry` does to a `Delete` that is NOT idempotent
//! (ledger #4063), against the published `ikigai-intray` and `ikigai-throttle` themselves.
//! Every finding is asserted, so this program fails when a module changes the behavior it
//! reports; `docs/research/sink-idempotency.md` §3 records its output.
//!
//! The fault is [`Lossy`]: an overlay under `Retry` whose endpoint runs the real take, so
//! the tuple is claimed and gone, then loses the answer and reports `Error::Timeout`, which
//! is what a remote mount, an IPC socket or a QUIC stream does when the connection drops
//! between the take and the reply. `Retry` sees a transient failure of a verb that
//! `Verb::is_idempotent` calls idempotent, and sends the take again.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Description, Endpoint, Error, Invocation, Iri, Kernel, Representation,
    Request, Resolution, Scope, Space, Verb,
};
use ikigai_intray::{CAP_OUT, CAP_READ, CAP_TAKE};
use ikigai_throttle::Retry;

// ------------------------------------------------------------------------ the fault

/// Shared between the overlay and the test: lose the next answer, and keep what was lost.
#[derive(Default)]
struct Fault {
    lose: AtomicBool,
    lost: Mutex<Vec<Vec<u8>>>,
    attempts: Mutex<usize>,
}

/// A space overlay whose endpoints act, then lose the answer once when told to.
struct Lossy {
    inner: Arc<dyn Space>,
    fault: Arc<Fault>,
}

impl Space for Lossy {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let fault = Arc::clone(&self.fault);
        self.inner.resolve(request, scope).map_endpoint(move |inner| {
            Arc::new(LossyEndpoint { inner, fault }) as Arc<dyn Endpoint>
        })
    }
}

struct LossyEndpoint {
    inner: Arc<dyn Endpoint>,
    fault: Arc<Fault>,
}

#[async_trait::async_trait]
impl Endpoint for LossyEndpoint {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn invoke(&self, inv: &Invocation<'_>) -> ikigai_core::Result<Representation> {
        *self.fault.attempts.lock().unwrap() += 1;
        let answer = self.inner.invoke(inv).await;
        if self.fault.lose.swap(false, Ordering::SeqCst) {
            if let Ok(repr) = &answer {
                self.fault.lost.lock().unwrap().push(repr.bytes.clone());
            }
            return Err(Error::Timeout(
                "the link dropped after the request was delivered".into(),
            ));
        }
        answer
    }

    fn describe(&self) -> Description {
        self.inner.describe()
    }
}

// --------------------------------------------------------------------------- fixtures

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "delete-idempotency-repro-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Fixture {
    /// Through `Retry(Lossy(intray))`: what a caller behind the throttle sees.
    retried: Kernel,
    /// Straight onto the same directory: what is really in the space.
    direct: Kernel,
    fault: Arc<Fault>,
    cap: Capability,
}

fn fixture(name: &str) -> Fixture {
    let dir = root(name);
    let fault = Arc::new(Fault::default());
    let lossy = Lossy {
        inner: Arc::new(ikigai_intray::space(dir.clone())),
        fault: Arc::clone(&fault),
    };
    let retry = Retry::new(lossy, 3).with_backoff(Duration::ZERO, Duration::ZERO);
    Fixture {
        retried: Kernel::new(Arc::new(retry)),
        direct: Kernel::new(Arc::new(ikigai_intray::space(dir))),
        fault,
        cap: Capability::scoped(vec![
            CAP_OUT.to_string(),
            CAP_READ.to_string(),
            CAP_TAKE.to_string(),
        ]),
    }
}

impl Fixture {
    fn out(&self, space: &str, content: &str) -> String {
        let request = Request::new(Verb::Sink, iri(space))
            .with_arg("content", ArgRef::Inline(content.as_bytes().to_vec()));
        text(&block_on(self.direct.issue(request, &self.cap)).unwrap())
    }

    /// The ids still pending in `space`, read past the fault.
    fn pending(&self, space: &str) -> usize {
        let listing = block_on(
            self.direct
                .issue(Request::new(Verb::Source, iri(space)), &self.cap),
        )
        .unwrap();
        text(&listing).lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// Send `request` through the throttle with the first answer lost.
    fn take_losing_the_answer(&self, request: Request) -> Result<String, Error> {
        *self.fault.attempts.lock().unwrap() = 0;
        self.fault.lose.store(true, Ordering::SeqCst);
        block_on(self.retried.issue(request, &self.cap)).map(|r| text(&r))
    }

    fn lost(&self) -> String {
        let lost = self.fault.lost.lock().unwrap();
        String::from_utf8_lossy(lost.last().expect("a lost answer")).into_owned()
    }

    fn attempts(&self) -> usize {
        *self.fault.attempts.lock().unwrap()
    }
}

fn text(repr: &Representation) -> String {
    String::from_utf8_lossy(&repr.bytes).into_owned()
}

// ----------------------------------------------------------------------------- cases

/// What the take's own contract says about sending it again.
fn the_contract() {
    let intray = ikigai_intray::space(root("contract"));
    let Resolution::Hit(hit) =
        intray.resolve(&Request::new(Verb::Meta, iri("urn:space:jobs")), &Scope::empty())
    else {
        panic!("urn:space:jobs did not resolve");
    };
    let take = hit
        .endpoint
        .describe()
        .actions
        .into_iter()
        .find(|a| a.verb == Verb::Delete)
        .expect("intray declares its take");
    assert!(take.resend.is_none());
    assert!(Verb::Delete.is_idempotent());
    println!("- intray take, its contract");
    println!(
        "    Verb::Delete.is_idempotent() = {}; the take declares resend: {:?}",
        Verb::Delete.is_idempotent(),
        take.resend
    );
    println!("    so throttle Retry treats every take as safe to send again");
}

/// A take with no selector is a FIFO pop: the retry pops the NEXT tuple.
fn take_with_no_selector() {
    let f = fixture("fifo");
    let space = "urn:space:jobs";
    f.out(space, "job 0");
    f.out(space, "job 1");
    assert_eq!(f.pending(space), 2);

    let answer = f
        .take_losing_the_answer(Request::new(Verb::Delete, iri(space)))
        .expect("the retry answers");
    let lost = f.lost();
    let pending = f.pending(space);
    assert_eq!(f.attempts(), 2, "throttle sent the take again");
    assert_ne!(answer, lost, "the retry took a different tuple");
    assert_eq!(pending, 0, "one call took both tuples");
    println!("- intray take with no selector, behind throttle Retry");
    println!("    attempts: {}", f.attempts());
    println!("    lost answer:  {lost}");
    println!("    retry answer: {answer}");
    println!("    tuples pending now: {pending} (one take was intended, which would leave one)");
    println!("    the lost tuple is gone from the space and was handed to nobody");
}

/// A take with `match=` takes the first match: the retry takes the NEXT match.
fn take_with_a_match() {
    let f = fixture("match");
    let space = "urn:space:people";
    f.out(space, "<urn:a> a <urn:Person> .");
    f.out(space, "<urn:b> a <urn:Person> .");
    let request = Request::new(Verb::Delete, iri(space)).with_arg(
        "match",
        ArgRef::Inline(b"ASK { ?s a <urn:Person> }".to_vec()),
    );
    let answer = f.take_losing_the_answer(request).expect("the retry answers");
    let lost = f.lost();
    let pending = f.pending(space);
    assert_eq!(f.attempts(), 2);
    assert_ne!(answer, lost);
    assert_eq!(pending, 0);
    println!("- intray take match=, behind throttle Retry");
    println!("    attempts: {}", f.attempts());
    println!("    lost answer:  {}", lost.trim());
    println!("    retry answer: {}", answer.trim());
    println!("    tuples pending now: {pending} (one take intended)");
}

/// A take by id: the retry cannot take another tuple, but answers `NotFound` for a take
/// that succeeded.
fn take_by_id() {
    let f = fixture("by-id");
    let space = "urn:space:q";
    let id = f.out(space, "only");
    let request = Request::new(Verb::Delete, iri(space))
        .with_arg("tuple", ArgRef::Inline(id.trim().as_bytes().to_vec()));
    let answer = f.take_losing_the_answer(request);
    let lost = f.lost();
    assert_eq!(f.attempts(), 2);
    assert!(matches!(answer, Err(Error::NotFound(_))), "{answer:?}");
    println!("- intray take tuple=<id>, behind throttle Retry");
    println!("    attempts: {}", f.attempts());
    println!("    lost answer:  {lost}");
    println!("    retry answer: {}", answer.unwrap_err());
    println!("    the take succeeded and the caller is told it found nothing");
}

fn main() {
    println!("Delete resends over a link that loses the answer after the take lands\n");
    the_contract();
    take_with_no_selector();
    take_with_a_match();
    take_by_id();
    println!("\n4 findings reproduced.");
}
