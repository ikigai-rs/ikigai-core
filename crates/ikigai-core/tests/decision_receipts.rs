//! **Kernel-emitted decision receipts, Deny as well as Allow** (ledger #4509;
//! `docs/design/decision-receipts.md`). Proposed, test-only: nothing in `src/` changes.
//!
//! The kernel's capability floor is ikigai's single decision point, and today it emits no
//! receipt. The nearest thing is the trace: a denial is recorded as a `TraceEvent` noted
//! `(DENIED_NOTE, <first lacking scope>)`. The first test pins why that cannot be a receipt:
//! the event names no verb, no arguments, only the FIRST scope the caller lacked and nothing
//! of the requirement set that decided it, so no third party could re-evaluate the verdict.
//!
//! The rest prove the note's decisions with a prototype built from public API alone: a
//! receipt per decision, chained by the previous receipt's hash, committing to the request,
//! the capability and the grant state; a verifier that re-evaluates the verdict with the
//! kernel's own predicate (`Description::unsatisfied_scopes`); and the two things a receipt
//! must never hold, a secret argument's value and a read's bytes. The prototype stands
//! OUTSIDE the kernel (it reads the verdict off the trace and the `Err` the call returned);
//! the design puts the same construction inside the floor, in the call that decides.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Error, Exact, FixedClock, FnEndpoint, Iri,
    Kernel, ReprType, Representation, Request, TraceEvent, Tracer, Verb, DENIED_NOTE,
};

const DOOR: &str = "urn:x:vault";
const READ: &str = "urn:cap:vault:read";
const AUDIT: &str = "urn:cap:vault:audit";
const SECRET_ARG: &[u8] = b"hunter2-api-token";
const SECRET_BYTES: &[u8] = b"the-vault-contents";
/// The instant the kernel's operational clock reads: the receipt's pinned instant.
const AT: u64 = 1_764_270_000_000;
/// The host's commitment key for argument values. Held by the host, never in a receipt.
const HOST_KEY: [u8; 32] = [7; 32];
/// The predicate the verdict was computed with. The crate version alone does not name it.
const FLOOR_RULE: &str = "ikigai.floor.v1";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// The door's declaration: a read needs two scopes.
fn description() -> Description {
    Description::new("vault")
        .verb(Verb::Source)
        .requires(READ)
        .requires(AUDIT)
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);

impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn kernel() -> (Kernel, Arc<Collect>) {
    let door = FnEndpoint::new("vault", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            SECRET_BYTES.to_vec(),
        ))
    })
    .with_description(description());
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new(DOOR), door)))
        .with_clock(Arc::new(FixedClock::at(AT)));
    let events = Arc::new(Collect::default());
    kernel.set_tracer(Arc::clone(&events) as Arc<dyn Tracer>);
    (kernel, events)
}

fn read_with_token() -> Request {
    Request::new(Verb::Source, iri(DOOR))
        .with_arg("token", ArgRef::Inline(SECRET_ARG.to_vec()))
        .with_arg("as", ArgRef::Inline(b"text/plain".to_vec()))
}

// ---------------------------------------------------------------------------------------
// The reproduction: what core emits today.
// ---------------------------------------------------------------------------------------

#[test]
fn today_the_denial_trace_cannot_be_re_evaluated() {
    let (kernel, events) = kernel();
    let caller = Capability::scoped(["urn:cap:other"]);
    let refused = block_on(kernel.issue(read_with_token(), &caller)).unwrap_err();
    let Error::Denied(denial) = refused else {
        panic!("expected a denial, got {refused:?}")
    };
    assert_eq!(denial.scopes(), [READ, AUDIT], "the kernel knew both");

    let events = events.0.lock().unwrap();
    let event = events
        .iter()
        .find(|e| e.target == DOOR)
        .expect("denial event");
    let denied: Vec<&str> = event
        .notes
        .iter()
        .filter(|(k, _)| k == DENIED_NOTE)
        .map(|(_, v)| v.as_str())
        .collect();
    // Only the first lacking scope; nothing of the requirement set, the verb, the arguments
    // or the kernel's version. A verifier holding this event cannot say whether a caller
    // holding READ alone would also have been refused, nor which request was refused.
    assert_eq!(denied, [READ]);
    assert!(!event.notes.iter().any(|(_, v)| v == AUDIT));
    let rendered = format!("{event:?}");
    assert!(
        !rendered.contains("Source"),
        "the event names no verb: {rendered}"
    );
    assert!(
        !rendered.contains("token"),
        "the event names no argument: {rendered}"
    );
    assert!(
        event.started.is_none(),
        "and no instant: the denial never ran"
    );
}

// ---------------------------------------------------------------------------------------
// The prototype.
// ---------------------------------------------------------------------------------------

type Digest = [u8; 32];

fn feed_str(h: &mut blake3::Hasher, s: &str) {
    feed_bytes(h, s.as_bytes());
}

fn feed_bytes(h: &mut blake3::Hasher, b: &[u8]) {
    h.update(&(b.len() as u64).to_le_bytes());
    h.update(b);
}

fn feed_list(h: &mut blake3::Hasher, items: &[String]) {
    h.update(&(items.len() as u64).to_le_bytes());
    for item in items {
        feed_str(h, item);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    Deny { lacking: Vec<String> },
}

/// One decision. Serialized as `Debug` here only to search it for what must be absent.
#[derive(Clone, Debug)]
struct Receipt {
    kernel: &'static str,
    floor_rule: &'static str,
    request: Digest,
    capability: Digest,
    grant_state: Digest,
    at: Option<u64>,
    prev: Digest,
    verdict: Verdict,
}

impl Receipt {
    fn hash(&self) -> Digest {
        let mut h = blake3::Hasher::new();
        feed_str(&mut h, "ikigai.receipt.v0");
        feed_str(&mut h, self.kernel);
        feed_str(&mut h, self.floor_rule);
        h.update(&self.request);
        h.update(&self.capability);
        h.update(&self.grant_state);
        match self.at {
            None => h.update(&[0]),
            Some(at) => h.update(&[1]).update(&at.to_le_bytes()),
        };
        h.update(&self.prev);
        match &self.verdict {
            Verdict::Allow => h.update(&[1]),
            Verdict::Deny { lacking } => {
                h.update(&[2]);
                feed_list(&mut h, lacking);
                &mut h
            }
        };
        *h.finalize().as_bytes()
    }
}

/// The request digest: verb, target, every argument NAME, `as=` in clear, and every other
/// argument value as a commitment keyed by the host. A plain hash of a low-entropy secret
/// (a PIN, a short token) is a dictionary away from the secret; a keyed one is not, and the
/// host can still open it for a party it chooses by disclosing the request and the key.
fn request_digest(request: &Request, key: &Digest) -> Digest {
    let mut h = blake3::Hasher::new();
    feed_str(&mut h, "ikigai.receipt.request.v0");
    h.update(&[request.verb as u8]);
    feed_str(&mut h, request.target.as_str());
    h.update(&(request.args.len() as u64).to_le_bytes());
    for (name, arg) in &request.args {
        feed_str(&mut h, name);
        // A held content's address is committed like a value: the address of a known
        // secret is itself a way to confirm it.
        let (tag, value): (u8, &[u8]) = match arg {
            ArgRef::Reference(iri) => (1, iri.as_str().as_bytes()),
            ArgRef::Inline(bytes) => (2, bytes),
            ArgRef::Content(id) => (3, id.as_bytes()),
        };
        h.update(&[tag]);
        if name == "as" {
            h.update(&[0]);
            feed_bytes(&mut h, value);
        } else {
            let mut keyed = blake3::Hasher::new_keyed(key);
            feed_str(&mut keyed, name);
            feed_bytes(&mut keyed, value);
            h.update(&[1]).update(keyed.finalize().as_bytes());
        }
    }
    *h.finalize().as_bytes()
}

/// The capability digest: the cache's capability fingerprint (`capability_key` in
/// `kernel.rs`), same encoding, untruncated. The cache keeps 64 bits of it; a receipt that
/// a third party relies on keeps all 256.
fn capability_digest(capability: &Capability) -> Digest {
    let mut h = blake3::Hasher::new();
    match capability.scopes() {
        None => {
            h.update(&[0]);
        }
        Some(scopes) => {
            h.update(&[1]);
            for scope in scopes {
                feed_str(&mut h, scope);
            }
        }
    }
    *h.finalize().as_bytes()
}

/// What decided the verdict, disclosed to a verifier: the endpoint the chain resolved to,
/// the verb, the floor's requirement set for it, and the host grant the request ran under.
#[derive(Clone, Debug)]
struct GrantState {
    endpoint: String,
    verb: Verb,
    requirements: Vec<String>,
    grant: Option<(String, Vec<String>)>,
}

impl GrantState {
    fn of(endpoint: &str, verb: Verb, description: &Description) -> Self {
        // Every requirement the floor applies to this verb: what an empty capability lacks.
        let none = Capability::scoped(Vec::<String>::new());
        GrantState {
            endpoint: endpoint.to_string(),
            verb,
            requirements: description.unsatisfied_scopes(verb, &none),
            grant: None,
        }
    }

    fn digest(&self) -> Digest {
        let mut h = blake3::Hasher::new();
        feed_str(&mut h, "ikigai.receipt.grant.v0");
        feed_str(&mut h, &self.endpoint);
        h.update(&[self.verb as u8]);
        feed_list(&mut h, &self.requirements);
        match &self.grant {
            None => {
                h.update(&[0]);
            }
            Some((prefix, scopes)) => {
                h.update(&[1]);
                feed_str(&mut h, prefix);
                feed_list(&mut h, scopes);
            }
        }
        *h.finalize().as_bytes()
    }

    /// Re-evaluate with the kernel's own predicate: a description declaring exactly the
    /// disclosed requirements, asked what `capability` lacks.
    fn evaluate(&self, capability: &Capability) -> Verdict {
        let declared = self
            .requirements
            .iter()
            .fold(Description::new("verifier").verb(self.verb), |d, r| {
                d.requires(r.as_str())
            });
        let lacking = declared.unsatisfied_scopes(self.verb, capability);
        if lacking.is_empty() {
            Verdict::Allow
        } else {
            Verdict::Deny { lacking }
        }
    }
}

/// The host-bound sink, prototyped: an append-only chain.
#[derive(Default)]
struct Chain(Vec<Receipt>);

impl Chain {
    fn head(&self) -> Digest {
        self.0.last().map(Receipt::hash).unwrap_or([0; 32])
    }
}

/// Issue `request` and write its receipt. The verdict is the kernel's: read off the
/// call's `Err(Denied)` and checked against the trace's denial note, never chosen by the
/// caller.
fn issue_receipted(
    kernel: &Kernel,
    events: &Collect,
    chain: &mut Chain,
    request: Request,
    capability: &Capability,
) -> Result<Representation, Error> {
    let state = GrantState::of(DOOR, request.verb, &description());
    let digest = request_digest(&request, &HOST_KEY);
    events.0.lock().unwrap().clear();
    let answer = block_on(kernel.issue(request, capability));
    let verdict = match &answer {
        Err(Error::Denied(denial)) => Verdict::Deny {
            lacking: denial.scopes().to_vec(),
        },
        _ => Verdict::Allow,
    };
    let noted = events
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|e| e.notes.iter().any(|(k, _)| k == DENIED_NOTE));
    assert_eq!(noted, matches!(verdict, Verdict::Deny { .. }));
    chain.0.push(Receipt {
        kernel: env!("CARGO_PKG_VERSION"),
        floor_rule: FLOOR_RULE,
        request: digest,
        capability: capability_digest(capability),
        grant_state: state.digest(),
        at: kernel.now().map(|t| t.as_millis()),
        prev: chain.head(),
        verdict,
    });
    answer
}

/// What a verifier is handed beside the chain, per receipt: the capability and grant state
/// the host discloses to it.
struct Disclosure {
    capability: Capability,
    state: GrantState,
}

#[derive(Debug, PartialEq, Eq)]
enum Rejected {
    BrokenChain(usize),
    CapabilityMismatch(usize),
    GrantStateMismatch(usize),
    VerdictDisagrees(usize),
}

fn verify(chain: &[Receipt], disclosed: &[Disclosure]) -> Result<(), Rejected> {
    let mut prev = [0; 32];
    for (i, (receipt, disclosure)) in chain.iter().zip(disclosed).enumerate() {
        if receipt.prev != prev {
            return Err(Rejected::BrokenChain(i));
        }
        if receipt.capability != capability_digest(&disclosure.capability) {
            return Err(Rejected::CapabilityMismatch(i));
        }
        if receipt.grant_state != disclosure.state.digest() {
            return Err(Rejected::GrantStateMismatch(i));
        }
        if receipt.verdict != disclosure.state.evaluate(&disclosure.capability) {
            return Err(Rejected::VerdictDisagrees(i));
        }
        prev = receipt.hash();
    }
    Ok(())
}

fn callers() -> [Capability; 3] {
    [
        Capability::scoped([READ, AUDIT]),
        Capability::scoped([READ]),
        Capability::scoped(["urn:cap:other"]),
    ]
}

fn run() -> (Chain, Vec<Disclosure>) {
    let (kernel, events) = kernel();
    let mut chain = Chain::default();
    let mut disclosed = Vec::new();
    for caller in callers() {
        let _ = issue_receipted(&kernel, &events, &mut chain, read_with_token(), &caller);
        disclosed.push(Disclosure {
            capability: caller,
            state: GrantState::of(DOOR, Verb::Source, &description()),
        });
    }
    (chain, disclosed)
}

#[test]
fn every_decision_gets_a_receipt_deny_included() {
    let (chain, _) = run();
    let verdicts: Vec<&Verdict> = chain.0.iter().map(|r| &r.verdict).collect();
    assert_eq!(
        verdicts,
        [
            &Verdict::Allow,
            &Verdict::Deny {
                lacking: vec![AUDIT.to_string()]
            },
            &Verdict::Deny {
                lacking: vec![READ.to_string(), AUDIT.to_string()]
            },
        ]
    );
    for receipt in &chain.0 {
        assert_eq!(
            receipt.at,
            Some(AT),
            "the pinned instant is the kernel's clock"
        );
        assert_eq!(receipt.kernel, env!("CARGO_PKG_VERSION"));
    }
    assert_eq!(chain.0[0].prev, [0; 32]);
    assert_eq!(chain.0[1].prev, chain.0[0].hash());
    assert_eq!(chain.0[2].prev, chain.0[1].hash());
    // Same request, three capabilities: one request digest, three capability digests.
    assert!(chain.0.iter().all(|r| r.request == chain.0[0].request));
    assert_ne!(chain.0[0].capability, chain.0[1].capability);
}

#[test]
fn a_verifier_re_evaluates_each_verdict_from_the_disclosed_grant_state() {
    let (chain, disclosed) = run();
    assert_eq!(verify(&chain.0, &disclosed), Ok(()));
}

#[test]
fn a_host_that_lies_about_the_grant_state_is_caught() {
    let (chain, mut disclosed) = run();
    // Claim the door required READ alone, so the second caller "should" have been allowed.
    disclosed[1].state.requirements = vec![READ.to_string()];
    assert_eq!(
        verify(&chain.0, &disclosed),
        Err(Rejected::GrantStateMismatch(1))
    );
}

#[test]
fn a_host_that_lies_about_the_capability_is_caught() {
    let (chain, mut disclosed) = run();
    disclosed[2].capability = Capability::scoped([READ, AUDIT]);
    assert_eq!(
        verify(&chain.0, &disclosed),
        Err(Rejected::CapabilityMismatch(2))
    );
}

#[test]
fn a_forged_verdict_disagrees_with_re_evaluation() {
    let (mut chain, disclosed) = run();
    // Rewrite the Deny as an Allow and re-link the chain behind it, as a writer holding
    // the sink could. The digests still match the disclosure; the verdict does not.
    chain.0[1].verdict = Verdict::Allow;
    chain.0[2].prev = chain.0[1].hash();
    assert_eq!(
        verify(&chain.0, &disclosed),
        Err(Rejected::VerdictDisagrees(1))
    );
}

#[test]
fn dropping_a_deny_breaks_the_chain() {
    let (mut chain, mut disclosed) = run();
    chain.0.remove(1);
    disclosed.remove(1);
    assert_eq!(verify(&chain.0, &disclosed), Err(Rejected::BrokenChain(1)));
}

#[test]
fn a_receipt_holds_no_secret_argument_and_no_read_bytes() {
    let (kernel, events) = kernel();
    let mut chain = Chain::default();
    let answer = issue_receipted(
        &kernel,
        &events,
        &mut chain,
        read_with_token(),
        &Capability::scoped([READ, AUDIT]),
    )
    .unwrap();
    assert_eq!(answer.bytes, SECRET_BYTES, "the read happened");

    let receipt = &chain.0[0];
    let rendered = format!("{receipt:?}");
    let raw: Vec<u8> = [
        &receipt.request[..],
        &receipt.capability,
        &receipt.grant_state,
        &receipt.prev,
    ]
    .concat();
    for needle in [SECRET_ARG, SECRET_BYTES] {
        assert!(!contains(rendered.as_bytes(), needle));
        assert!(!contains(&raw, needle));
    }
    // No plain hash of the secret either: the request digest moves with the host's key,
    // so a dictionary over tokens cannot be checked against a published receipt.
    assert_ne!(
        request_digest(&read_with_token(), &HOST_KEY),
        request_digest(&read_with_token(), &[9; 32])
    );
    // And the digest does change when the secret does: it still commits to the request.
    let other = Request::new(Verb::Source, iri(DOOR))
        .with_arg("token", ArgRef::Inline(b"another".to_vec()))
        .with_arg("as", ArgRef::Inline(b"text/plain".to_vec()));
    assert_ne!(receipt.request, request_digest(&other, &HOST_KEY));
}

#[test]
fn argument_order_is_not_identity_in_the_request_digest() {
    let a = Request::new(Verb::Source, iri(DOOR))
        .with_arg("as", ArgRef::Inline(b"text/plain".to_vec()))
        .with_arg("token", ArgRef::Inline(SECRET_ARG.to_vec()));
    assert_eq!(
        request_digest(&a, &HOST_KEY),
        request_digest(&read_with_token(), &HOST_KEY)
    );
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
