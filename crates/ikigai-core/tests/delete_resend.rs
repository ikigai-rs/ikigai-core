//! **A Delete that is not idempotent** (ledger #4063). `Verb::is_idempotent` says every
//! `Delete` is, and `ikigai-throttle`'s `Retry` sends one again on a transient failure. An
//! intray take with no selector is a FIFO pop: sent again after its answer was lost, it pops
//! the NEXT tuple, so one call consumes two and the first is handed to nobody
//! (`docs/research/sink-idempotency/repro-delete/` reproduces that against the published
//! intray and throttle). The contract can now say so: `ActionSpec::resend_unsafe`, read
//! through `ActionSpec::may_resend` / `Description::may_resend`, which keep the verb's
//! default for a Delete that declares nothing.
//!
//! One test per claim: the pop, resent by the verb's rule, takes two and by the contract's
//! rule takes one; an undeclared Delete is still resent; the declaration round-trips through
//! the JSON (Meta) face and moves the contract digest; a read cannot declare it; and
//! `urn:kernel:actions` says `ik:resendSafe false`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ActionSpec, ArgRef, ArgSpec, Capability, Description, EndpointSpace, Error, Exact, FnEndpoint,
    Iri, Kernel, ReprType, Representation, Request, Resend, Resolution, Scope, Space, Verb,
};

const QUEUE: &str = "urn:space:jobs";

/// A work queue: `Delete` pops the oldest job and answers it. After `lose` is set, the next
/// pop still pops, and then its answer is lost and the caller sees a `Timeout`, as over a
/// link that drops between the write and the reply.
struct Queue {
    jobs: Arc<Mutex<VecDeque<&'static str>>>,
    lose: Arc<AtomicBool>,
    kernel: Kernel,
    space: Arc<EndpointSpace>,
}

fn queue(take: ActionSpec) -> Queue {
    let jobs = Arc::new(Mutex::new(VecDeque::from(["job 0", "job 1"])));
    let lose = Arc::new(AtomicBool::new(false));
    let (popped, dropped) = (Arc::clone(&jobs), Arc::clone(&lose));
    let endpoint = FnEndpoint::new("space", move |_| {
        let job = popped
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| Error::NotFound("the queue is empty".into()))?;
        if dropped.swap(false, Ordering::SeqCst) {
            return Err(Error::Timeout("the link dropped after the pop".into()));
        }
        Ok(Representation::new(
            ReprType::new("text/plain"),
            job.as_bytes().to_vec(),
        ))
    })
    .with_description(Description::new("space").verb(Verb::Source).action(take));
    let space = Arc::new(EndpointSpace::new().bind(Exact::new(QUEUE), endpoint));
    Queue {
        jobs,
        lose,
        kernel: Kernel::new(Arc::clone(&space) as Arc<dyn Space>),
        space,
    }
}

impl Queue {
    /// Pop once with the answer lost, and send again while `may_resend` allows (at most
    /// three attempts): the loop a retrying overlay runs. Answers the attempts made and the
    /// final result.
    fn pop_losing_the_answer(
        &self,
        may_resend: impl Fn(&Description) -> bool,
    ) -> (usize, Result<String, Error>) {
        self.lose.store(true, Ordering::SeqCst);
        let request = Request::new(Verb::Delete, Iri::parse(QUEUE).unwrap());
        let description = self.describe();
        let mut attempts = 0;
        loop {
            attempts += 1;
            match block_on(self.kernel.issue(request.clone(), &Capability::root())) {
                Err(e) if e.is_transient() && may_resend(&description) && attempts < 3 => {}
                other => return (attempts, other.map(|r| String::from_utf8(r.bytes).unwrap())),
            }
        }
    }

    /// The take's contract, as the endpoint the request resolves to describes it.
    fn describe(&self) -> Description {
        let meta = Request::new(Verb::Meta, Iri::parse(QUEUE).unwrap());
        let Resolution::Hit(hit) = self.space.resolve(&meta, &Scope::empty()) else {
            panic!("{QUEUE} did not resolve");
        };
        hit.endpoint.describe()
    }

    fn left(&self) -> usize {
        self.jobs.lock().unwrap().len()
    }
}

fn pop() -> ActionSpec {
    ActionSpec::new(Verb::Delete).summary("take the oldest job (a FIFO pop)")
}

#[test]
fn a_pop_resent_by_its_verb_takes_two_and_by_its_contract_takes_one() {
    // The defect: by the verb alone, a Delete is resent, and the pop pops the next job.
    let by_verb = queue(pop().resend_unsafe());
    let (attempts, answer) = by_verb.pop_losing_the_answer(|_| Verb::Delete.is_idempotent());
    assert_eq!(attempts, 2);
    assert_eq!(answer.unwrap(), "job 1", "the retry popped the NEXT job");
    assert_eq!(
        by_verb.left(),
        0,
        "one call consumed both jobs; job 0 went to nobody"
    );

    // The contract says the pop is not idempotent, and the caller who asks it stops.
    let by_contract = queue(pop().resend_unsafe());
    let (attempts, answer) =
        by_contract.pop_losing_the_answer(|d| d.may_resend(Verb::Delete, |_| false));
    assert_eq!(attempts, 1);
    assert!(matches!(answer, Err(Error::Timeout(_))), "{answer:?}");
    assert_eq!(
        by_contract.left(),
        1,
        "job 1 is still there for the next taker"
    );
}

#[test]
fn a_delete_that_declares_nothing_keeps_its_verbs_default() {
    let undeclared = queue(pop());
    let description = undeclared.describe();
    assert!(description.may_resend(Verb::Delete, |_| false));
    let (attempts, _) = undeclared.pop_losing_the_answer(|d| d.may_resend(Verb::Delete, |_| false));
    assert_eq!(attempts, 2, "an undeclared Delete is resent, as before");

    // A flat description with no action for its Delete reads the same way.
    assert!(Description::new("file")
        .verb(Verb::Delete)
        .may_resend(Verb::Delete, |_| false));
    // And a Sink is still not resent without a declaration.
    assert!(!Description::new("post")
        .verb(Verb::Sink)
        .may_resend(Verb::Sink, |_| true));
    assert!(
        Verb::Delete.is_idempotent(),
        "the verb's own answer is unchanged"
    );
}

#[test]
fn a_keyed_delete_is_resent_only_with_its_key() {
    let take = ActionSpec::new(Verb::Delete)
        .input(ArgSpec::new("key").optional())
        .resend_safe_with_key("key");
    assert!(take.may_resend(|name| name == "key"));
    assert!(!take.may_resend(|_| false));
}

#[test]
fn unsafe_round_trips_through_the_json_meta_face_and_moves_the_digest() {
    let description = queue(pop().resend_unsafe()).describe();
    let json = serde_json::to_value(&description).unwrap();
    let take = json["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["verb"] == "Delete")
        .unwrap();
    assert_eq!(take["resend"], serde_json::json!("unsafe"));
    let back: ActionSpec = serde_json::from_value(take.clone()).unwrap();
    assert_eq!(back.resend, Some(Resend::Unsafe));
    assert_eq!(back.resend.as_ref().and_then(Resend::key), None);
    assert!(!back.resend_is_safe(|_| true));

    let ids = [
        pop().contract_iri("space"),
        pop().resend_unsafe().contract_iri("space"),
        pop().resend_safe().contract_iri("space"),
    ];
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
}

#[test]
fn a_read_cannot_declare_a_resend_unsafe() {
    assert!(Description::new("space")
        .action(pop().resend_unsafe())
        .validate()
        .is_ok());
    assert!(Description::new("post")
        .action(ActionSpec::new(Verb::Sink).resend_unsafe())
        .validate()
        .is_ok());
    let read = Description::new("space").action(ActionSpec::new(Verb::Source).resend_unsafe());
    let err = read.validate().unwrap_err().to_string();
    assert!(err.contains("always safe to send again"), "{err}");
}

#[test]
fn the_action_manifold_says_a_delete_is_not_safe_to_resend() {
    let q = queue(pop().resend_unsafe());
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:actions").unwrap())
        .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
    let turtle = String::from_utf8(
        block_on(q.kernel.issue(request, &Capability::root()))
            .unwrap()
            .bytes,
    )
    .unwrap();
    let row = |verb: &str| {
        let subject = format!("<urn:ikigai:match:{verb}:urn:space:jobs>");
        let start = turtle
            .find(&subject)
            .unwrap_or_else(|| panic!("{subject} in {turtle}"));
        let end = start + turtle[start..].find(" .\n").unwrap();
        turtle[start..end].to_string()
    };
    assert!(row("delete").contains("ik:resendSafe false"), "{turtle}");
    assert!(!row("source").contains("resendSafe"), "{turtle}");
}
