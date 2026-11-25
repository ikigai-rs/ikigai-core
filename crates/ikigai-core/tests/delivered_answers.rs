//! **Delivered-answer registration and race recovery for push** (ledger #4400; design:
//! `docs/design/push.md`, "Delivered answers: the record and its handshake").
//!
//! A DESIGN SPIKE, test-only: the [`prototype`] module below is not API, and nothing in
//! `src/` changes. It is built only from what core exports today, which is itself part of
//! the finding: a host can already keep an honest record of what it delivered, given one
//! cut listener that hears every cut (`ListenSpec::prefix("")` under root). It cannot do so
//! with a listener under the session's own capability, because that listener hears a thread
//! only while a RESIDENT entry under the same capability depends on it.
//!
//! The first three tests reproduce the gap the note describes, on today's kernel:
//! - a listener matching a composite's target IRI misses a cut of a differently named part;
//! - a session-capability listener does not hear a cut to an uncached delivered answer;
//! - nor to a delivered answer the cache has since evicted.
//!
//! The rest prove the prototype: each transition (read, install, replace, remove) raced
//! by a cut, a composite, one target under different arguments and corridors, uncached and
//! evicted answers, failed and canceled deliveries, bounds and degraded coverage, deadlines,
//! revocation and teardown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, CacheBound, Capability, Clock, EndpointSpace, Exact, Expiry,
    FnEndpoint, Invocation, Iri, Kernel, ListenSpec, Lru, ReprType, Representation, Request, Scope,
    Time, Verb, CAP_LISTEN,
};

use prototype::{Coverage, Deadline, Installed, Limits, Subscription, ViewKey};

/// The prototype: a host-owned subscription that records what one session was handed.
///
/// Who owns what (the decisions in push.md):
/// - The HOST owns the record's lifetime: only the host knows an answer reached the page
///   (it calls [`Subscription::install`] after its write succeeded), and dropping the
///   subscription is the whole of teardown, as dropping a `CutListener` is today.
/// - The KERNEL owns the facts: the threads and expiry on the representation it returned,
///   and the single cut order its listener queue delivers in. The listener queue is the
///   linearization point of the handshake: a cut is queued before `Kernel::cut` returns.
/// - No timer and no clock live here: [`Subscription::next_deadline`] reports the earliest
///   operational deadline and [`Subscription::expire`] takes the operational `now` the host
///   read when its own timer woke (time.md: waking is a host service).
pub mod prototype {
    use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
    use std::fmt;

    use ikigai_core::{
        Capability, CutListener, Expiry, Kernel, ListenSpec, Representation, Request, RequestId,
        Scope, Thread, Time,
    };

    /// Everything that makes two reads the same read. A bare target IRI is not enough:
    /// `greet name=ada` and `greet name=bob`, or the same read in two corridors, carry
    /// different threads and show different values.
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct ViewKey {
        /// Target, verb and every argument (order-free), `as=` included: [`Request::id`].
        request: RequestId,
        /// The resolution context: the chain's fingerprint (0 for the empty chain).
        corridor: u64,
        /// The capability partition the read ran under.
        partition: String,
    }

    impl ViewKey {
        /// The key of `request` read in `scope` under `capability`.
        pub fn of(request: &Request, scope: &Scope, capability: &Capability) -> Self {
            ViewKey {
                request: request.id(),
                corridor: scope.fingerprint(),
                partition: partition(capability),
            }
        }
    }

    /// A stand-in for the cache key's capability fingerprint, which is crate-private.
    fn partition(capability: &Capability) -> String {
        match capability.scopes() {
            None => "root".to_string(),
            Some(scopes) => scopes.iter().cloned().collect::<Vec<_>>().join(" "),
        }
    }

    /// Bounds on everything the subscription holds.
    #[derive(Clone, Copy, Debug)]
    pub struct Limits {
        /// Delivered views recorded at once.
        pub views: usize,
        /// Thread edges summed over every recorded view.
        pub edges: usize,
        /// Reads registered and not yet installed or abandoned.
        pub in_flight: usize,
        /// Cuts remembered while a read is in flight (the race window's log).
        pub race_log: usize,
        /// The underlying cut listener's queue.
        pub queue: usize,
    }

    impl Default for Limits {
        fn default() -> Self {
            Limits {
                views: 64,
                edges: 1024,
                in_flight: 16,
                race_log: 256,
                queue: 256,
            }
        }
    }

    /// Whether the subscription can vouch that every stale view will be reported.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Coverage {
        /// Every recorded view is tracked and no cut was missed.
        Complete,
        /// Something was lost: the host must re-read every view it shows
        /// ([`Subscription::refresh`]) and may not claim complete push coverage.
        Degraded(String),
    }

    /// The next time the host's timer must wake.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Deadline {
        /// No fresh view expires: no timer is needed.
        None,
        /// The earliest operational deadline of a fresh view.
        At(Time),
        /// A fresh view was uncacheable (`Expiry::Always`): no safe future deadline, so
        /// it is due now, and the host chooses a rate-limited polling fallback.
        Poll,
    }

    /// What installing a delivered answer recorded.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Installed {
        /// Recorded; no cut raced it.
        Fresh,
        /// Recorded, but a cut of one of its threads raced the read or the install (or
        /// cuts were lost meanwhile): it is already queued as stale, so the host re-reads.
        Stale,
        /// The answer is visible but the subscription is full: it is not tracked, the old
        /// record for the view is gone, and coverage is degraded until the view is removed.
        Untracked(String),
    }

    /// A refusal, in words that name what was wrong.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Refusal(pub String);

    impl fmt::Display for Refusal {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    /// One registered read, handed back to [`Subscription::install`] or
    /// [`Subscription::abandon`]. Not `Clone`: one read, one outcome.
    #[derive(Debug)]
    pub struct Ticket {
        key: ViewKey,
        generation: u64,
    }

    struct Pending {
        key: ViewKey,
        presentation: String,
        /// The cut-log position when the read was registered: every cut heard from here on
        /// raced this read.
        watermark: u64,
        /// Cuts were lost while this read was in flight: it cannot be shown fresh.
        doubtful: bool,
    }

    struct Record {
        presentation: String,
        threads: BTreeSet<Thread>,
        expiry: Expiry,
        generation: u64,
        stale: bool,
    }

    /// One session's subscription: what it was delivered, and which of it went stale.
    pub struct Subscription {
        listener: CutListener,
        partition: String,
        limits: Limits,
        revoked: bool,
        next_generation: u64,
        /// How many cuts this subscription has taken from its listener: the watermark.
        heard: u64,
        log: VecDeque<(u64, Thread)>,
        pending: HashMap<u64, Pending>,
        records: HashMap<ViewKey, Record>,
        /// Thread to the delivered views carrying it: the subscription-local reverse map.
        reverse: HashMap<Thread, HashSet<ViewKey>>,
        edges: usize,
        /// Presentation keys gone stale since the host last asked, each once.
        notices: Vec<String>,
        /// Views handed out but not recorded (a bound was full).
        untracked: HashSet<ViewKey>,
        lost: Option<String>,
    }

    impl Subscription {
        /// Open a subscription for a session reading under `session`. The host hears
        /// cuts through its own root listener; nothing heard is disclosed except the
        /// presentation keys of views this session was delivered.
        pub fn open(
            kernel: &Kernel,
            session: &Capability,
            limits: Limits,
        ) -> Result<Self, Refusal> {
            if !session.allows(ikigai_core::CAP_LISTEN) {
                return Err(Refusal(format!(
                    "denied: the session's capability does not grant `{}`, which a \
                     subscription requires",
                    ikigai_core::CAP_LISTEN
                )));
            }
            let listener = kernel
                .listen(
                    ListenSpec::new().prefix("").capacity(limits.queue),
                    &Capability::root(),
                )
                .map_err(|e| Refusal(e.to_string()))?;
            Ok(Subscription {
                listener,
                partition: partition(session),
                limits,
                revoked: false,
                next_generation: 1,
                heard: 0,
                log: VecDeque::new(),
                pending: HashMap::new(),
                records: HashMap::new(),
                reverse: HashMap::new(),
                edges: 0,
                notices: Vec::new(),
                untracked: HashSet::new(),
                lost: None,
            })
        }

        fn live(&self) -> Result<(), Refusal> {
            if self.revoked {
                return Err(Refusal(
                    "denied: this subscription's capability was revoked; open a new \
                     subscription under the current capability and re-read every view"
                        .to_string(),
                ));
            }
            Ok(())
        }

        /// Register a read BEFORE issuing it. `presentation` is the host's opaque name
        /// for the part of the page the answer will fill.
        pub fn register(&mut self, key: ViewKey, presentation: &str) -> Result<Ticket, Refusal> {
            self.live()?;
            if key.partition != self.partition {
                return Err(Refusal(
                    "denied: this read runs under a different capability than the \
                     subscription's; register it with a subscription opened under that \
                     capability"
                        .to_string(),
                ));
            }
            self.pump();
            if self.pending.len() >= self.limits.in_flight {
                return Err(Refusal(format!(
                    "too many reads in flight: {} are registered and not yet installed or \
                     abandoned, the subscription's bound; finish or abandon one first",
                    self.pending.len()
                )));
            }
            let generation = self.next_generation;
            self.next_generation += 1;
            self.pending.insert(
                generation,
                Pending {
                    key: key.clone(),
                    presentation: presentation.to_string(),
                    watermark: self.heard,
                    doubtful: false,
                },
            );
            Ok(Ticket { key, generation })
        }

        /// Record `answer` as what the view now shows. Call it only after the answer
        /// was handed out: until then the previous record (its edges and its deadline)
        /// stays, since that is still what the page shows.
        pub fn install(
            &mut self,
            ticket: Ticket,
            answer: &Representation,
        ) -> Result<Installed, Refusal> {
            self.live()?;
            self.pump();
            let Some(pending) = self.pending.remove(&ticket.generation) else {
                return Err(Refusal(
                    "conflict: this read is no longer registered: its view was removed, or \
                     the subscription was refreshed, while the answer was in flight"
                        .to_string(),
                ));
            };
            if let Some(current) = self.records.get(&ticket.key) {
                if current.generation > ticket.generation {
                    self.trim_log();
                    return Err(Refusal(
                        "conflict: a newer answer for this view was already delivered; this \
                         older one does not replace it"
                            .to_string(),
                    ));
                }
            }
            let threads = answer.threads().clone();
            let raced = pending.doubtful
                || self
                    .log
                    .iter()
                    .any(|(at, thread)| *at >= pending.watermark && threads.contains(thread));
            self.trim_log();

            // The answer is now what the page shows: the old record is obsolete either way.
            self.drop_record(&ticket.key);
            let over_views = self.records.len() >= self.limits.views;
            let over_edges = self.edges + threads.len() > self.limits.edges;
            if over_views || over_edges {
                let why = if over_views {
                    format!(
                        "the subscription already records {} views, its bound",
                        self.records.len()
                    )
                } else {
                    format!(
                        "recording {} more threads would pass the subscription's bound of {} \
                         edges",
                        threads.len(),
                        self.limits.edges
                    )
                };
                if self.untracked.len() < self.limits.views {
                    self.untracked.insert(ticket.key);
                } else {
                    self.lost = Some(format!("views were delivered untracked: {why}"));
                }
                return Ok(Installed::Untracked(why));
            }
            for thread in &threads {
                self.reverse
                    .entry(thread.clone())
                    .or_default()
                    .insert(ticket.key.clone());
            }
            self.edges += threads.len();
            self.untracked.remove(&ticket.key);
            self.records.insert(
                ticket.key,
                Record {
                    presentation: pending.presentation.clone(),
                    threads,
                    expiry: answer.expiry,
                    generation: ticket.generation,
                    stale: raced,
                },
            );
            if raced {
                self.notices.push(pending.presentation);
                Ok(Installed::Stale)
            } else {
                Ok(Installed::Fresh)
            }
        }

        /// The read failed or was canceled: nothing reached the page, so the record of
        /// the answer still visible (if any) is untouched, staleness included.
        pub fn abandon(&mut self, ticket: Ticket) {
            self.pending.remove(&ticket.generation);
            self.trim_log();
        }

        /// The page no longer shows the view: forget its record and any read of it still
        /// in flight.
        pub fn remove(&mut self, key: &ViewKey) {
            self.pump();
            if let Some(record) = self.records.get(key) {
                let gone = record.presentation.clone();
                self.notices.retain(|p| *p != gone);
            }
            self.drop_record(key);
            self.untracked.remove(key);
            self.pending.retain(|_, p| &p.key != key);
            self.trim_log();
        }

        /// The session's authority is gone. Everything is forgotten, undrained notices
        /// included, and every later call refuses.
        pub fn revoke(&mut self) {
            self.revoked = true;
            self.pending.clear();
            self.records.clear();
            self.reverse.clear();
            self.notices.clear();
            self.untracked.clear();
            self.log.clear();
            self.edges = 0;
            let _ = self.listener.drain();
        }

        /// Presentation keys that went stale since the last call, each once.
        pub fn take_stale(&mut self) -> Result<Vec<String>, Refusal> {
            self.live()?;
            self.pump();
            Ok(std::mem::take(&mut self.notices))
        }

        /// Whether the subscription can vouch for every view.
        pub fn coverage(&mut self) -> Coverage {
            self.pump();
            if let Some(lost) = &self.lost {
                return Coverage::Degraded(lost.clone());
            }
            if !self.untracked.is_empty() {
                return Coverage::Degraded(format!(
                    "{} delivered views are not tracked; remove them or re-read them once \
                     there is room",
                    self.untracked.len()
                ));
            }
            Coverage::Complete
        }

        /// Begin a full refresh after degraded coverage: every recorded view is marked
        /// stale and returned, reads in flight cannot install fresh, and coverage of lost
        /// cuts is restored from here on (untracked views still count against it).
        pub fn refresh(&mut self) -> Result<Vec<String>, Refusal> {
            self.live()?;
            self.pump();
            for pending in self.pending.values_mut() {
                pending.doubtful = true;
            }
            let mut all: Vec<String> = Vec::new();
            for record in self.records.values_mut() {
                record.stale = true;
                all.push(record.presentation.clone());
            }
            all.sort();
            self.notices.clear();
            self.lost = None;
            Ok(all)
        }

        /// When the host's timer must next wake, judged over fresh views only (a stale
        /// view is already owed a re-read, so it does not fire again).
        pub fn next_deadline(&self) -> Deadline {
            let fresh = self.records.values().filter(|r| !r.stale);
            let mut earliest: Option<Time> = None;
            for record in fresh {
                match record.expiry {
                    Expiry::Always => return Deadline::Poll,
                    Expiry::Never => {}
                    Expiry::At(at) => {
                        earliest = Some(earliest.map_or(at, |e| if at < e { at } else { e }))
                    }
                }
            }
            earliest.map_or(Deadline::None, Deadline::At)
        }

        /// The host's timer woke and it read operational time `now`: mark every fresh
        /// view whose deadline is at or before `now` stale. An uncacheable view is due.
        pub fn expire(&mut self, now: Time) {
            for record in self.records.values_mut() {
                let due = match record.expiry {
                    Expiry::Always => true,
                    Expiry::Never => false,
                    Expiry::At(at) => at <= now,
                };
                if due && !record.stale {
                    record.stale = true;
                    self.notices.push(record.presentation.clone());
                }
            }
        }

        /// How many thread edges are recorded (for the bound tests).
        pub fn edges(&self) -> usize {
            self.edges
        }

        /// Whether the view is recorded and fresh, recorded and stale, or not recorded.
        pub fn state(&self, key: &ViewKey) -> Option<bool> {
            self.records.get(key).map(|r| !r.stale)
        }

        fn drop_record(&mut self, key: &ViewKey) {
            if let Some(old) = self.records.remove(key) {
                for thread in &old.threads {
                    if let Some(keys) = self.reverse.get_mut(thread) {
                        keys.remove(key);
                        if keys.is_empty() {
                            self.reverse.remove(thread);
                        }
                    }
                }
                self.edges -= old.threads.len();
            }
        }

        /// Keep only the cuts some in-flight read could still have raced.
        fn trim_log(&mut self) {
            match self.pending.values().map(|p| p.watermark).min() {
                None => self.log.clear(),
                Some(oldest) => {
                    while self.log.front().is_some_and(|(at, _)| *at < oldest) {
                        self.log.pop_front();
                    }
                }
            }
        }

        fn mark_stale(&mut self, key: &ViewKey) {
            if let Some(record) = self.records.get_mut(key) {
                if !record.stale {
                    record.stale = true;
                    self.notices.push(record.presentation.clone());
                }
            }
        }

        /// Take every cut the listener queued, in the kernel's cut order.
        fn pump(&mut self) {
            if self.revoked {
                return;
            }
            let batch = self.listener.drain();
            for event in batch.events {
                let at = self.heard;
                self.heard += 1;
                if !self.pending.is_empty() {
                    if self.log.len() >= self.limits.race_log {
                        self.log.clear();
                        for pending in self.pending.values_mut() {
                            pending.doubtful = true;
                        }
                        self.lose(format!(
                            "more than {} cuts arrived while reads were in flight, the race \
                             log's bound",
                            self.limits.race_log
                        ));
                    } else {
                        self.log.push_back((at, event.thread.clone()));
                    }
                }
                let keys: Vec<ViewKey> = self
                    .reverse
                    .get(&event.thread)
                    .map(|keys| keys.iter().cloned().collect())
                    .unwrap_or_default();
                for key in keys {
                    self.mark_stale(&key);
                }
            }
            if batch.dropped > 0 {
                for pending in self.pending.values_mut() {
                    pending.doubtful = true;
                }
                self.lose(format!(
                    "the cut queue was full and {} cuts were missed",
                    batch.dropped
                ));
            }
        }

        fn lose(&mut self, why: String) {
            let keys: Vec<ViewKey> = self.records.keys().cloned().collect();
            for key in keys {
                self.mark_stale(&key);
            }
            self.lost = Some(why);
        }
    }
}

// ---------------------------------------------------------------------------------------
// The world the tests read.

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

#[derive(Clone)]
struct TestClock(Arc<AtomicU64>);

impl Clock for TestClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

/// Which row `urn:switch` reads: row 3 until a test flips it, then row 2.
static SWITCHED: AtomicBool = AtomicBool::new(false);

type Hook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// The session: may read and listen.
fn session() -> Capability {
    Capability::scoped([CAP_LISTEN, "urn:cap:example:session"])
}

/// - `urn:row:{1,2,3}`: cacheable rows (each hangs from its own name).
/// - `urn:total`: a composite of rows 1 and 2, under neither row's name.
/// - `urn:greet name=…`: reads `urn:person:{name}`; one target, two argument values.
/// - `urn:live`: UNCACHED, hangs from `urn:row:3`.
/// - `urn:quote`: cacheable until 1 500 on the kernel clock.
/// - `urn:switch`: reads row 3, or row 2 once [`SWITCHED`]: its threads move.
/// - `urn:racy`: reads, then runs the hook (a cut mid-read), then answers.
fn world(hook: Hook) -> EndpointSpace {
    let mut space = EndpointSpace::new();
    for row in ["1", "2", "3"] {
        let body = format!("row-{row}");
        space = space.bind(
            Exact::new(format!("urn:row:{row}")),
            FnEndpoint::new("row", move |_| Ok(text(&body).cacheable())),
        );
    }
    for person in ["ada", "bob"] {
        space = space.bind(
            Exact::new(format!("urn:person:{person}")),
            FnEndpoint::new("person", move |_| Ok(text(person).cacheable())),
        );
    }
    let total = AsyncFnEndpoint::new("total", |inv: &Invocation<'_>| {
        Box::pin(async move {
            let a = inv.source(&iri("urn:row:1")).await?;
            let b = inv.source(&iri("urn:row:2")).await?;
            let mut body = a.bytes;
            body.extend(b.bytes);
            Ok(Representation::new(ReprType::new("text/plain"), body).cacheable())
        })
    });
    let greet = AsyncFnEndpoint::new("greet", |inv: &Invocation<'_>| {
        Box::pin(async move {
            let name = String::from_utf8_lossy(inv.inline_arg("name")?).to_string();
            let who = inv.source(&iri(&format!("urn:person:{name}"))).await?;
            Ok(Representation::new(ReprType::new("text/plain"), who.bytes).cacheable())
        })
    });
    let live = FnEndpoint::new("live", |_| Ok(text("now").depends_on("urn:row:3")));
    let quote = FnEndpoint::new("quote", |inv: &Invocation<'_>| {
        let _ = inv.now();
        Ok(text("quote").cacheable_until(Time::from_millis(1_500)))
    });
    let racy = FnEndpoint::new("racy", move |_| {
        let fire = hook.lock().unwrap().take();
        if let Some(fire) = fire {
            fire();
        }
        Ok(text("read before the cut").cacheable())
    });
    let switch = AsyncFnEndpoint::new("switch", |inv: &Invocation<'_>| {
        Box::pin(async move {
            let row = if SWITCHED.load(Ordering::SeqCst) {
                "urn:row:2"
            } else {
                "urn:row:3"
            };
            let part = inv.source(&iri(row)).await?;
            Ok(Representation::new(ReprType::new("text/plain"), part.bytes).cacheable())
        })
    });
    space
        .bind(Exact::new("urn:total"), total)
        .bind(Exact::new("urn:switch"), switch)
        .bind(Exact::new("urn:greet"), greet)
        .bind(Exact::new("urn:live"), live)
        .bind(Exact::new("urn:quote"), quote)
        .bind(Exact::new("urn:racy"), racy)
}

fn kernel_with(hook: Hook, cache_entries: usize) -> Arc<Kernel> {
    let clock = TestClock(Arc::new(AtomicU64::new(1_000)));
    Arc::new(
        Kernel::new(Arc::new(world(hook)))
            .with_clock(Arc::new(clock))
            .with_cache_policy(Arc::new(Lru::with_bound(CacheBound::new(
                cache_entries,
                1 << 20,
            )))),
    )
}

fn kernel() -> Arc<Kernel> {
    kernel_with(Hook::default(), 4096)
}

/// Register, read, hand out, install: the whole handshake for one view.
fn deliver(
    sub: &mut Subscription,
    kernel: &Kernel,
    request: Request,
    presentation: &str,
) -> (ViewKey, Installed) {
    let cap = session();
    let key = ViewKey::of(&request, &Scope::empty(), &cap);
    let ticket = sub.register(key.clone(), presentation).unwrap();
    let answer = block_on(kernel.issue(request, &cap)).unwrap();
    (key, sub.install(ticket, &answer).unwrap())
}

fn open(kernel: &Kernel) -> Subscription {
    Subscription::open(kernel, &session(), Limits::default()).unwrap()
}

// ---------------------------------------------------------------------------------------
// The gap, reproduced on today's kernel.

#[test]
fn reproduce_a_target_matched_listener_misses_a_composites_differently_named_part() {
    let kernel = kernel();
    let root = Capability::root();
    let by_target = kernel
        .listen(ListenSpec::new().exact("urn:total"), &root)
        .unwrap();
    let answer = block_on(kernel.issue(source("urn:total"), &root)).unwrap();
    assert!(answer.threads().iter().any(|t| t.as_str() == "urn:row:2"));
    kernel.cut("urn:row:2");
    assert!(
        by_target.drain().is_empty(),
        "the cut invalidated urn:total, but a listener on its name heard nothing"
    );
}

#[test]
fn reproduce_a_session_listener_does_not_hear_an_uncached_delivered_answer() {
    let kernel = kernel();
    let cap = session();
    let own = kernel
        .listen(ListenSpec::new().prefix("urn:"), &cap)
        .unwrap();
    let answer = block_on(kernel.issue(source("urn:live"), &cap)).unwrap();
    assert_eq!(answer.expiry, Expiry::Always);
    assert!(answer.threads().iter().any(|t| t.as_str() == "urn:row:3"));
    kernel.cut("urn:row:3");
    assert!(
        own.drain().is_empty(),
        "the page shows an answer carrying urn:row:3, yet its listener heard no cut"
    );
}

#[test]
fn reproduce_a_session_listener_does_not_hear_an_evicted_delivered_answer() {
    let kernel = kernel_with(Hook::default(), 1);
    let cap = session();
    let own = kernel
        .listen(ListenSpec::new().prefix("urn:"), &cap)
        .unwrap();
    block_on(kernel.issue(source("urn:row:1"), &cap)).unwrap();
    block_on(kernel.issue(source("urn:row:2"), &cap)).unwrap();
    assert!(!kernel.is_cached(&source("urn:row:1"), &cap), "evicted");
    kernel.cut("urn:row:1");
    assert!(
        own.drain().is_empty(),
        "the page still shows row 1, yet its listener heard no cut once it was evicted"
    );
}

// ---------------------------------------------------------------------------------------
// The prototype.

#[test]
fn a_composite_is_stale_when_any_part_is_cut_whatever_its_name() {
    let kernel = kernel();
    let mut sub = open(&kernel);
    let (key, installed) = deliver(&mut sub, &kernel, source("urn:total"), "total-cell");
    assert_eq!(installed, Installed::Fresh);
    kernel.cut("urn:row:3");
    assert!(sub.take_stale().unwrap().is_empty(), "row 3 is not a part");
    kernel.cut("urn:row:2");
    assert_eq!(sub.take_stale().unwrap(), ["total-cell"]);
    assert_eq!(sub.state(&key), Some(false));
    kernel.cut("urn:row:1");
    assert!(
        sub.take_stale().unwrap().is_empty(),
        "already owed a re-read: coalesced"
    );
}

#[test]
fn one_target_under_different_arguments_and_corridors_is_different_views() {
    let kernel = kernel();
    let cap = session();
    let mut sub = open(&kernel);
    let ada = source("urn:greet").with_arg("name", ArgRef::Inline(b"ada".to_vec()));
    let bob = source("urn:greet").with_arg("name", ArgRef::Inline(b"bob".to_vec()));
    let (ka, _) = deliver(&mut sub, &kernel, ada, "greet-ada");
    let (kb, _) = deliver(&mut sub, &kernel, bob, "greet-bob");
    assert_ne!(ka, kb, "one target IRI, two views");
    kernel.cut("urn:person:bob");
    assert_eq!(sub.take_stale().unwrap(), ["greet-bob"]);
    assert_eq!(sub.state(&ka), Some(true));

    // The same read of urn:total, in a corridor that rebinds row 1 onto another thread.
    let alt = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:row:1"),
        FnEndpoint::new("alt-row", |_| {
            Ok(text("alt").cacheable().depends_on("urn:alt:1"))
        }),
    ));
    let scope = || Scope::empty().with_named(iri("urn:ctx:alt"), alt.clone());
    let plain = ViewKey::of(&source("urn:total"), &Scope::empty(), &cap);
    let inside = ViewKey::of(&source("urn:total"), &scope(), &cap);
    assert_ne!(plain, inside, "one request, two corridors, two views");
    let t1 = sub.register(plain.clone(), "total-root").unwrap();
    let a1 = block_on(kernel.issue(source("urn:total"), &cap)).unwrap();
    sub.install(t1, &a1).unwrap();
    let t2 = sub.register(inside.clone(), "total-alt").unwrap();
    let a2 = block_on(kernel.issue_in(source("urn:total"), &cap, scope())).unwrap();
    assert!(a2.threads().iter().any(|t| t.as_str() == "urn:alt:1"));
    sub.install(t2, &a2).unwrap();
    kernel.cut("urn:alt:1");
    assert_eq!(sub.take_stale().unwrap(), ["total-alt"]);
    assert_eq!(sub.state(&plain), Some(true));
}

#[test]
fn uncached_and_evicted_delivered_answers_are_still_heard() {
    let kernel = kernel_with(Hook::default(), 1);
    let cap = session();
    let mut sub = open(&kernel);
    deliver(&mut sub, &kernel, source("urn:live"), "live");
    deliver(&mut sub, &kernel, source("urn:row:1"), "row-1");
    deliver(&mut sub, &kernel, source("urn:row:2"), "row-2");
    assert!(
        !kernel.is_cached(&source("urn:row:1"), &cap),
        "row 1 evicted"
    );
    kernel.cut("urn:row:3");
    kernel.cut("urn:row:1");
    let mut stale = sub.take_stale().unwrap();
    stale.sort();
    assert_eq!(stale, ["live", "row-1"]);
    assert_eq!(sub.coverage(), Coverage::Complete);
}

#[test]
fn a_cut_racing_the_read_installs_the_answer_as_stale() {
    let hook = Hook::default();
    let kernel = kernel_with(Arc::clone(&hook), 4096);
    let weak = Arc::downgrade(&kernel);
    *hook.lock().unwrap() = Some(Box::new(move || {
        weak.upgrade().unwrap().cut("urn:racy");
    }));
    let mut sub = open(&kernel);
    let (key, installed) = deliver(&mut sub, &kernel, source("urn:racy"), "racy");
    assert_eq!(
        installed,
        Installed::Stale,
        "computed before the cut it raced"
    );
    assert!(
        !kernel.is_cached(&source("urn:racy"), &session()),
        "and the kernel declined to cache it"
    );
    assert_eq!(sub.take_stale().unwrap(), ["racy"]);
    assert_eq!(sub.state(&key), Some(false));
}

#[test]
fn a_cut_between_the_read_and_the_install_installs_the_answer_as_stale() {
    let kernel = kernel();
    let cap = session();
    let mut sub = open(&kernel);
    let key = ViewKey::of(&source("urn:total"), &Scope::empty(), &cap);
    let ticket = sub.register(key, "total").unwrap();
    let answer = block_on(kernel.issue(source("urn:total"), &cap)).unwrap();
    kernel.cut("urn:row:3"); // not a part: harmless
    assert!(sub.take_stale().unwrap().is_empty());
    kernel.cut("urn:row:1"); // a part, while the answer is on its way to the page
    assert_eq!(sub.install(ticket, &answer).unwrap(), Installed::Stale);
    assert_eq!(sub.take_stale().unwrap(), ["total"]);

    // And a cut BEFORE registration is not a race: the read saw it.
    let key = ViewKey::of(&source("urn:row:2"), &Scope::empty(), &cap);
    kernel.cut("urn:row:2");
    let ticket = sub.register(key, "row-2").unwrap();
    let answer = block_on(kernel.issue(source("urn:row:2"), &cap)).unwrap();
    assert_eq!(sub.install(ticket, &answer).unwrap(), Installed::Fresh);
}

#[test]
fn a_replacement_swaps_edges_only_once_the_new_answer_is_delivered() {
    let kernel = kernel();
    let cap = session();
    let mut sub = open(&kernel);
    let ada = source("urn:greet").with_arg("name", ArgRef::Inline(b"ada".to_vec()));
    let (key, _) = deliver(&mut sub, &kernel, ada.clone(), "greet");
    // The same view re-read: while in flight the old record (and its edges) stays.
    let ticket = sub.register(key.clone(), "greet").unwrap();
    let answer = block_on(kernel.issue(ada.clone(), &cap)).unwrap();
    kernel.cut("urn:person:ada");
    assert_eq!(
        sub.take_stale().unwrap(),
        ["greet"],
        "the visible old answer is stale"
    );
    assert_eq!(
        sub.install(ticket, &answer).unwrap(),
        Installed::Stale,
        "and so is the new one, which the cut raced"
    );
    assert_eq!(
        sub.take_stale().unwrap(),
        ["greet"],
        "owed again: one more re-read"
    );

    // `urn:switch` reads row 3, then (once switched) row 2: the same view's threads move.
    // A cut of a thread only the OLD answer carried is owed by the old answer alone.
    let (key2, _) = deliver(&mut sub, &kernel, source("urn:switch"), "panel");
    SWITCHED.store(true, Ordering::SeqCst);
    kernel.cut("urn:switch"); // the host's own write: the old answer is owed a re-read
    assert_eq!(sub.take_stale().unwrap(), ["panel"]);
    let ticket = sub.register(key2.clone(), "panel").unwrap();
    let answer = block_on(kernel.issue(source("urn:switch"), &cap)).unwrap();
    assert!(answer.threads().iter().all(|t| t.as_str() != "urn:row:3"));
    assert!(
        sub.take_stale().unwrap().is_empty(),
        "coalesced while still owed"
    );
    kernel.cut("urn:row:3"); // only the old, visible answer carried it
    assert_eq!(sub.install(ticket, &answer).unwrap(), Installed::Fresh);
    assert_eq!(
        sub.state(&key2),
        Some(true),
        "the new answer did not carry row 3"
    );
    kernel.cut("urn:row:3");
    assert!(
        sub.take_stale().unwrap().is_empty(),
        "the old edge was replaced"
    );
    kernel.cut("urn:row:2");
    assert_eq!(sub.take_stale().unwrap(), ["panel"], "the new edge is live");
}

#[test]
fn a_failed_or_canceled_delivery_keeps_the_record_of_the_visible_answer() {
    let kernel = kernel();
    let cap = session();
    let mut sub = open(&kernel);
    let (key, _) = deliver(&mut sub, &kernel, source("urn:total"), "total");
    let edges = sub.edges();

    // Failed: the read errored, nothing reached the page.
    let ticket = sub.register(key.clone(), "total").unwrap();
    assert!(block_on(kernel.issue(source("urn:nowhere"), &cap)).is_err());
    sub.abandon(ticket);
    assert_eq!(sub.edges(), edges);
    assert_eq!(sub.state(&key), Some(true));

    // Canceled after a newer delivery overtook it: the older one may not replace it.
    let older = sub.register(key.clone(), "total").unwrap();
    let newer = sub.register(key.clone(), "total").unwrap();
    let a = block_on(kernel.issue(source("urn:total"), &cap)).unwrap();
    sub.install(newer, &a).unwrap();
    let refused = sub.install(older, &a).unwrap_err();
    assert_eq!(
        refused.to_string(),
        "conflict: a newer answer for this view was already delivered; this older one does \
         not replace it"
    );
    kernel.cut("urn:row:1");
    assert_eq!(
        sub.take_stale().unwrap(),
        ["total"],
        "the visible answer is still heard"
    );
}

#[test]
fn a_removed_view_hears_nothing_and_its_read_in_flight_cannot_install() {
    let kernel = kernel();
    let cap = session();
    let mut sub = open(&kernel);
    let (key, _) = deliver(&mut sub, &kernel, source("urn:total"), "total");
    let ticket = sub.register(key.clone(), "total").unwrap();
    let answer = block_on(kernel.issue(source("urn:total"), &cap)).unwrap();
    kernel.cut("urn:row:1"); // races the removal
    sub.remove(&key);
    assert_eq!(sub.edges(), 0);
    let refused = sub.install(ticket, &answer).unwrap_err();
    assert!(refused
        .to_string()
        .starts_with("conflict: this read is no longer registered"));
    kernel.cut("urn:row:2");
    assert_eq!(sub.state(&key), None);
    assert!(
        sub.take_stale().unwrap().is_empty(),
        "a removed view owes no re-read"
    );
}

#[test]
fn the_session_learns_only_about_its_own_deliveries() {
    let kernel = kernel();
    let mut mine = open(&kernel);
    let mut theirs = open(&kernel); // a second session sharing the same capability
    deliver(&mut mine, &kernel, source("urn:row:1"), "mine");
    deliver(&mut theirs, &kernel, source("urn:row:2"), "theirs");
    kernel.cut("urn:row:2");
    kernel.cut("urn:bindings:unrelated");
    assert!(mine.take_stale().unwrap().is_empty());
    assert_eq!(theirs.take_stale().unwrap(), ["theirs"]);

    // A read under another capability is not this subscription's to record.
    let other = Capability::scoped(["urn:cap:example:other"]);
    let key = ViewKey::of(&source("urn:row:1"), &Scope::empty(), &other);
    assert_eq!(
        mine.register(key, "x").unwrap_err().to_string(),
        "denied: this read runs under a different capability than the subscription's; \
         register it with a subscription opened under that capability"
    );
    let refused = Subscription::open(&kernel, &other, Limits::default())
        .err()
        .unwrap();
    assert_eq!(
        refused.to_string(),
        "denied: the session's capability does not grant `urn:cap:kernel:listen`, which a \
         subscription requires"
    );
}

#[test]
fn every_bound_holds_and_a_lost_record_degrades_coverage_until_a_full_refresh() {
    let kernel = kernel();
    let cap = session();
    let small = Limits {
        views: 2,
        edges: 8,
        in_flight: 1,
        race_log: 2,
        queue: 2,
    };
    let mut sub = Subscription::open(&kernel, &cap, small).unwrap();
    deliver(&mut sub, &kernel, source("urn:row:1"), "r1");
    deliver(&mut sub, &kernel, source("urn:row:2"), "r2");
    // Views: the third is handed out but cannot be recorded.
    let (k3, installed) = deliver(&mut sub, &kernel, source("urn:row:3"), "r3");
    assert_eq!(
        installed,
        Installed::Untracked("the subscription already records 2 views, its bound".into())
    );
    assert!(matches!(sub.coverage(), Coverage::Degraded(_)));
    sub.remove(&k3);
    assert_eq!(
        sub.coverage(),
        Coverage::Complete,
        "the untracked view is gone"
    );

    // In flight.
    let key = ViewKey::of(&source("urn:total"), &Scope::empty(), &cap);
    let ticket = sub.register(key.clone(), "t").unwrap();
    assert_eq!(
        sub.register(key.clone(), "t").unwrap_err().to_string(),
        "too many reads in flight: 1 are registered and not yet installed or abandoned, the \
         subscription's bound; finish or abandon one first"
    );
    // The race log: more cuts than it holds while the read is in flight.
    for n in 0..3 {
        kernel.cut(format!("urn:noise:{n}"));
        let _ = sub.take_stale().unwrap();
    }
    assert_eq!(
        sub.coverage(),
        Coverage::Degraded(
            "more than 2 cuts arrived while reads were in flight, the race log's bound".into()
        )
    );
    let answer = block_on(kernel.issue(source("urn:total"), &cap)).unwrap();
    sub.remove(&ViewKey::of(&source("urn:row:2"), &Scope::empty(), &cap));
    assert_eq!(
        sub.install(ticket, &answer).unwrap(),
        Installed::Stale,
        "cuts were lost meanwhile: never fresh"
    );
    assert_eq!(sub.refresh().unwrap(), ["r1", "t"]);
    assert_eq!(sub.coverage(), Coverage::Complete);

    // The listener's queue: cuts beyond it are missed between drains.
    for n in 0..5 {
        kernel.cut(format!("urn:noise:{n}"));
    }
    assert_eq!(
        sub.coverage(),
        Coverage::Degraded("the cut queue was full and 3 cuts were missed".into())
    );
    assert_eq!(sub.refresh().unwrap(), ["r1", "t"]);

    // Edges: a delivery whose threads would pass the bound is untracked.
    let mut tight = Subscription::open(
        &kernel,
        &cap,
        Limits {
            edges: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    let (_, installed) = deliver(&mut tight, &kernel, source("urn:total"), "t");
    assert_eq!(
        installed,
        Installed::Untracked(
            "recording 3 more threads would pass the subscription's bound of 2 edges".into()
        )
    );
}

#[test]
fn deadlines_are_reported_for_a_host_timer_and_fire_once() {
    let kernel = kernel();
    let mut sub = open(&kernel);
    assert_eq!(sub.next_deadline(), Deadline::None);
    deliver(&mut sub, &kernel, source("urn:row:1"), "row");
    assert_eq!(
        sub.next_deadline(),
        Deadline::None,
        "Never contributes no timer"
    );
    let (_, installed) = deliver(&mut sub, &kernel, source("urn:quote"), "quote");
    assert_eq!(installed, Installed::Fresh);
    assert_eq!(sub.next_deadline(), Deadline::At(Time::from_millis(1_500)));
    sub.expire(Time::from_millis(1_499));
    assert!(sub.take_stale().unwrap().is_empty(), "not early");
    sub.expire(Time::from_millis(1_500));
    assert_eq!(sub.take_stale().unwrap(), ["quote"], "due at the deadline");
    sub.expire(Time::from_millis(1_600));
    assert!(
        sub.take_stale().unwrap().is_empty(),
        "not again before a new delivery"
    );
    assert_eq!(sub.next_deadline(), Deadline::None);
    deliver(&mut sub, &kernel, source("urn:live"), "live");
    assert_eq!(
        sub.next_deadline(),
        Deadline::Poll,
        "uncached: no safe future deadline"
    );
}

#[test]
fn revocation_forgets_everything_and_refuses_in_words() {
    let kernel = kernel();
    let cap = session();
    let mut sub = open(&kernel);
    let (key, _) = deliver(&mut sub, &kernel, source("urn:row:1"), "row");
    kernel.cut("urn:row:1"); // queued, not yet drained: must not be disclosed afterward
    sub.revoke();
    assert_eq!(sub.state(&key), None);
    let refusal = sub.take_stale().unwrap_err().to_string();
    assert_eq!(
        refusal,
        "denied: this subscription's capability was revoked; open a new subscription under \
         the current capability and re-read every view"
    );
    let key = ViewKey::of(&source("urn:row:2"), &Scope::empty(), &cap);
    assert_eq!(sub.register(key, "x").unwrap_err().to_string(), refusal);

    // Teardown is dropping it; the kernel keeps serving and cutting.
    drop(sub);
    kernel.cut("urn:row:1");
    assert!(block_on(kernel.issue(source("urn:row:1"), &cap)).is_ok());
}
