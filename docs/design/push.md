# Push: listening to handed-out answers

Status: design proposal; no runtime behavior changes. Evidence is core
[`1d11151`](https://github.com/ikigai-rs/ikigai-core/tree/1d111516aa7e390110a8ad1007e96576966eb90a)
and the public tutorial at
[`37a36c1`](https://github.com/ikigai-rs/ikigai-tutorial/tree/37a36c18787ab58e120722ccf2a4e8c93826b205).
For clock domains and host-provided waking, use the sibling [time proposal](https://github.com/ikigai-rs/ikigai-core/pull/337); this note does not prescribe a clock implementation.

## Evidence and boundary

The tutorial's [time chapter](https://github.com/ikigai-rs/ikigai-tutorial/blob/37a36c18787ab58e120722ccf2a4e8c93826b205/books/ikigai/src/applied/spreadsheet-4.md), [feed chapter](https://github.com/ikigai-rs/ikigai-tutorial/blob/37a36c18787ab58e120722ccf2a4e8c93826b205/books/ikigai/src/applied/spreadsheet-5.md), and [push chapter](https://github.com/ikigai-rs/ikigai-tutorial/blob/37a36c18787ab58e120722ccf2a4e8c93826b205/books/ikigai/src/applied/spreadsheet-7.md) demonstrate a page that remembers each drawn view's threads and deadline, listens for cuts, and sets a timer at the earliest deadline. Its `NOW()` example needs a redraw even when nothing writes. Core's [`composite_revalidation` test](../../crates/ikigai-core/tests/composite_revalidation.rs) independently checks that expiry causes recomputation while the cut listener remains empty. Expiry is not a cut.

Core already has [`urn:kernel:dependents`](../../crates/ikigai-core/src/kernel_ops.rs): the older claim that there is no such operation is stale. It is inspect-gated, reports live *resident cache entries* at a snapshot, and currently scans their edges ([implementation](../../crates/ikigai-core/src/cache.rs)). It is neither a reverse index nor a record of what a particular page received. Eviction, an uncacheable answer, a previous cut, or another request can make the cache differ from the screen. A listener's bounded `invalidated` targets have the same limitation. The tutorial's statement that there is no index remains accurate at these revisions.

[`ListenSpec`](../../crates/ikigai-core/src/listen.rs) matches exact thread names and prefixes, not the target of a composite answer. A composite target can depend on differently named threads, so matching its IRI as a thread misses those cuts; subscribing to an empty prefix hears all eligible cuts and makes each matching cut scan resident edges. A representation exposes its effective expiry and threads to Rust callers; the kernel has no general named listener or answer-metadata resource. The tutorial implements its own named wrapper and `carries` read. These observations establish the design gap, not a measurement of its performance.

## Proposed model

A host owns a **subscription** for each session, under the exact capability used for its reads, and records each answer actually handed out. A view key identifies the complete read: target, verb, arguments and representation choice, resolution context or corridor, and capability partition. A bare target IRI is insufficient when two reads of it differ. The host may associate a separate opaque presentation key with that read so it can redraw the right part of a page. Never infer that an entry resident in the shared cache is a delivered view.

A handed-out record contains that key, effective threads, effective expiry, and a generation identifying the delivery. Register before the first read, then atomically install the record with a cut watermark or validate generations after installation; if a cut races the read or installation, mark the view stale and retry or resynchronize. Replacing a view replaces its old edges and deadline only after the new answer is safely handed out. Removing a view removes its record. A failed or canceled delivery must not silently replace the record for the answer still visible. Scope this protocol to a subscription, not globally to all cache entries.

### Cuts and reverse lookup

Offer target-matched subscriptions in addition to today's thread exact/prefix filters. A target match means a cut to *any thread carried by the registered delivered read*, not merely a cut whose thread equals the target. Keep a subscription-local reverse map from thread to delivered view keys, plus forward records for replacement and removal. A cut looks up candidates and queues affected presentation keys; shared thread-to-subscription indexing may avoid scanning every listener, subject to measurement. The cache may separately maintain a thread-to-resident-entry reverse index to accelerate its current dependent enumeration. These indexes answer different questions and must not be substituted for each other.

Maintain indexes transactionally on insert, replacement, expiry, eviction, removal, and subscription teardown; guard against stale keys and generation reuse. Bound subscriptions, views, edges, and event queues. If an index cannot record an answer, report degraded coverage and require a full refresh or polling; never claim complete push coverage. Define ordering with cuts and the delivery watermark, avoid reentrant callbacks under the cache lock, and wake only after locks are released. Benchmark writer latency, memory per edge and listener, replacement/eviction churn, and fan-out at realistic cardinalities against today's scan before choosing an index layout.

### Deadlines as events

For the delivered records, expose each effective deadline and the minimum future deadline. A host-supplied timer can wake at that minimum and enqueue a `deadline` event naming all due view keys, then rearm after redraw and replacement. `Expiry::Never` contributes no timer; `Expiry::Always` gives no safe future deadline and requires immediate revalidation or a host-chosen polling fallback, with rate limiting to avoid a busy loop. A past deadline is due immediately. If no operational clock or timer exists, report that deadline delivery is unavailable; do not promise a wakeup or silently use a different clock. Recheck the operational clock on wake, including clock corrections, and reschedule if necessary. A fixed historical read must follow the expiry actually returned, not invent a live-time refresh.

A deadline event means an answer may be stale, not that its value changed. Coalesce duplicate cut and deadline notifications for a view without losing the obligation to refresh. Use generation tokens to ignore obsolete timer entries after replacement; do not continually emit the same deadline before a new delivery. Bound timer state and report overflow as a resync requirement. Timer suspension, restart, connection loss, and queue overflow require a full refresh; no exactly-once or durable replay is promised. The host must redraw outside the writer's stack and rearm after the resulting delivery.

### Named resources and authority

Propose singular, keyed names, subject to vocabulary review: `urn:kernel:listener:id:{id}` for one subscription and `urn:kernel:answer:id:{id}` for one recorded delivery. Each `id` is an opaque, unguessable handle generated when the subscription or delivery record is created; these names are illustrative, not a finalized IRI grammar. The bare set `urn:kernel:listener` lists the caller's listeners, and `urn:kernel:answer` lists the caller's recorded deliveries; both require `urn:cap:kernel:listen` and are scoped to the creating session and exact read capability. A listener resource creates/configures, drains or waits for, and deletes one bounded subscription; an answer resource exposes only its own effective threads, expiry, and delivery identity. Define verb, representation, lifetime, collision, restart, and error semantics before implementation. Metadata must describe the same read and delivery, not reissue a potentially different request as a side effect. The Rust API and resource face should share one contract; a wire adapter can translate events without exposing Rust types.

Require `urn:cap:kernel:listen` and bind each resource to the creating session and exact read capability. Recheck authority at access and on revocation; prevent handle guessing or a second session with a different authority from draining or modifying it. Disclose only metadata and view keys authorized by that session's delivered reads, including on deadline events, overflow, and errors. Existing listeners filter non-root hearing using resident cache entries under the same capability fingerprint; an explicit delivered-answer record must preserve that isolation without relying on residency. Root's broad hearing is not a substitute for session authority. Clarify capability changes, equal fingerprints shared by sessions, and resource cleanup on disconnect in the implementation contract.

### Host and transport

A server can forward stale notifications over SSE or WebSocket and let the page re-read its views. Subscribe before the initial delivery, apply the race protocol above, and refresh all displayed views after reconnect, dropped events, truncated names, lost state, or an uncertain delivery. Use bounded queues, connection limits, idle cleanup and backpressure; close and resynchronize a slow consumer rather than silently omit updates. This is an invalidation channel, not a promise that recomputation changes bytes. Same-value writes and corridor-wide cuts can cause identical redraws; early cutoff is separate work (#613), requiring an explicit equality and authority contract. Polling remains a fallback when listening, metadata, or timers are unavailable.

## Delivered answers: the record and its handshake

Status: proposed in ledger #4400 (a design spike, the first follow-up below); its decisions are proposals until a maintainer accepts them. The proof is a test-only prototype in [`tests/delivered_answers.rs`](../../crates/ikigai-core/tests/delivered_answers.rs); it is not API, and nothing in `src/` changed. It is built only from what core exports today.

**What was reproduced first.** Three tests in that file pin the gap on today's kernel. A listener whose `ListenSpec` names a composite's target hears nothing when a differently named part is cut. A listener under the session's own capability hears nothing when a thread of an *uncached* answer it was handed is cut, and nothing once a cached answer it was handed has been *evicted*. Both follow from hearing being filtered by resident entries under the same capability fingerprint. So a non-root host cannot build an honest record from a session-capability listener today. The prototype instead hears through one root listener with an empty prefix and discloses only the presentation keys of the session's own records (see the authority decision below).

### The view key

A view key is the triple (request identity, chain fingerprint, capability partition). The request identity is `Request::id`: verb, target and every argument without regard to order, `as=` included, so the representation choice is part of it. The chain fingerprint is `Scope::fingerprint`, the resolution context or corridor, `0` for the empty chain. The capability partition is the cache key's own partition; the prototype stands in for the crate-private fingerprint with the sorted scope set. A bare target IRI is not enough: `greet name=ada` and `greet name=bob` carry different threads, and so does one read of `urn:total` in a corridor that rebinds a part. Cutting one must flag only that view, and the tests check this. The host keeps its own opaque presentation key beside the view key to know what to redraw.

### The record and the handshake

A record holds the view key, the presentation key, the answer's effective threads (`Representation::threads`, which already unions every part) and effective expiry, a generation, and a stale flag. Generations come from a per-subscription counter and are never reused. A subscription also keeps a reverse map from each thread to the view keys carrying it, and a count of edges.

The handshake has four steps:

1. **Register** the read before issuing it. The host gets a ticket holding a fresh generation and the *watermark*: how many cuts the subscription has taken from its listener so far.
2. **Issue** the read.
3. **Hand out** the answer to the page.
4. **Install** it with the ticket, only after the hand-out succeeded. Install first takes every queued cut. If any cut taken at or after the ticket's watermark names a thread the answer carries, the answer is installed **stale** and queued for a re-read. Otherwise it is installed fresh. Install never refuses a raced answer, because that answer is what the page now shows, and refusing it would leave a visible answer unrecorded.

The linearization point is the listener queue. `Kernel::cut` queues its event under the cache lock before it returns, so each cut is in exactly one of three places:

- before the watermark, so the read saw it;
- between the watermark and install, so the install sees it in the race log;
- after install, so the reverse map flags the record.

A cut inside the endpoint while it is reading is the second case, and the kernel already declines to cache that result. Tests race a cut against the read (from inside the endpoint), against the install, against a replacement and against a removal.

**Why a watermark rather than validating generations after install.** Core has no public per-thread generation read. `urn:kernel:threads` is inspect-gated, and it tracks a thread only while something resident depends on it. That is exactly what is missing for the uncached and evicted answers this record exists to cover. The watermark needs no residency, only the cut order the listener already delivers.

### Replacement, removal, failure and teardown

- **Replacement** is a re-read of the same view key. Until its install, the old record keeps its edges and its deadline, because the old answer is still on the page. A cut of a thread only the old answer carried flags the old answer, and the new answer then installs fresh. Install removes the old edges and adds the new ones in one step.
- **Removal** forgets the record, its edges, any read of the view still in flight (whose later install is refused with a conflict), and any notice not yet taken.
- **A failed or canceled delivery** calls abandon, which leaves the record of the visible answer untouched, including its stale flag. An older ticket that arrives after a newer one was installed is refused: ``conflict: a newer answer for this view was already delivered; this older one does not replace it``.
- **Teardown** is dropping the subscription, as dropping a `CutListener` is today.

### Bounds and degraded coverage

Everything is bounded: views, edges, reads in flight, the race log, and the listener queue.

- A register past the in-flight bound is refused in words that give the count and the bound.
- An answer that does not fit the view or edge bound is still on the page, so install reports it *untracked*, with the bound that was full. Coverage stays degraded until the host removes that view.
- An overflow of the race log or of the listener queue marks every record stale. It also makes every read in flight install stale, and reports `Coverage::Degraded` with what was lost.
- `refresh` returns every recorded presentation key for a full re-read and restores coverage of lost cuts from that point.

Coverage is never reported complete while anything delivered is untracked. A stale view is notified once until a new delivery, so duplicate cuts and deadlines coalesce.

### Decisions

**The host owns the record's lifetime; the kernel owns the facts and the order.** Only the host knows whether an answer reached the page. A write can fail after `issue` returns, a connection can close, or the host can decide not to draw. A kernel-owned registry would have to be told all of this anyway, and would sit on the cut path under the cache lock. The kernel supplies what only it knows: each answer's effective threads and expiry, and one cut order delivered through a bounded queue that the host drains outside the writer's stack. This agrees with the tutorial's host-owned implementation and with today's listeners, which are host-held values. The record type may still ship in core as a plain library type that the host holds and drives. In that case the kernel should add a hearing path that does not depend on residency, keyed by the threads the session was handed. A non-root subscription would then need no root listener, and an empty-prefix listener would not queue every cut. Measure that before choosing its layout (the third follow-up).

**Revocation is the host's act, and it forgets everything.** A `Capability` is a value, and the kernel has no event when one is revoked. So the host revokes the subscription. Revoking drops every record, edge, read in flight and undrained notice, so no cut is disclosed after authority is gone. Every later call refuses with ``denied: this subscription's capability was revoked; open a new subscription under the current capability and re-read every view``. Narrowed authority is a different partition, and the partition is part of the view key. So records never migrate across partitions: the host opens a new subscription and refreshes in full. Register checks that the read's partition is the subscription's own, and opening checks `urn:cap:kernel:listen`. Two sessions sharing one capability hold two subscriptions, and a test checks that neither hears the other's views.

**Timer injection lives outside the record, in the host, as time.md says.** The record reads no clock and owns no timer. `next_deadline` reports the earliest effective deadline of a fresh view, in operational time (cache expiry's domain), or `Poll` when a fresh view was uncacheable (`Expiry::Always`, which has no safe future deadline). `Expiry::Never` contributes nothing. The host arms its own waking service (time.md's paired monotonic clock and timer). When the timer fires, the host reads operational time, rechecking it after any clock correction, and passes it to `expire`. `expire` marks each view due at or before that time stale, the deadline itself included, and never earlier. A stale view no longer contributes a deadline, so the same deadline is not emitted again before a new delivery. A host with no operational clock or timer reports that deadline delivery is unavailable; the record never substitutes a clock.

**Still open.** The keyed listener and answer resources and their wire format, the reverse-index layout and its measurement, and transport are the remaining follow-ups. The prototype serializes calls through `&mut self`. The race it closes is between deliveries and cuts, and two hosts' threads installing the same subscription at once must hold the host's own lock.

## Validation and open decisions

A prototype should exercise a composite with differently named dependencies; distinct arguments and corridors for one target; uncached and evicted delivered answers; replacement and deletion; simultaneous cut/read/install; expiry with no cut; equal and past deadlines; clock jumps; no timer; queue overflow and reconnect; two isolated capabilities and two sessions sharing a capability; revocation; and resource teardown. Assert no missed stale view, no cross-session disclosure, and bounded recovery, then measure costs under load. The delivered-record lifetime, the atomic delivery handshake, capability revocation semantics and timer injection are decided in the section above; the resource wire format remains open before committing to an API. The tutorial demonstrates one host-owned implementation; it does not establish that a kernel-owned registry is universally preferable.

## Proposed follow-up items

- **Define delivered-answer registration and race recovery** (designed in ledger #4400, above). Specify view identity, atomic read-to-delivery handoff, replacement, teardown, and bounded fallback; test cuts racing each transition.
- **Add target-matched subscriptions.** Match cuts through delivered answer threads, preserve per-session authority, and test composite targets, nonresident answers, overflow, and resynchronization.
- **Measure and implement reverse indexes.** Compare resident-cache and delivered-view indexes with scans under fan-out and churn; maintain each through its full lifecycle only if the measured tradeoff warrants it.
- **Deliver deadline notifications.** Expose effective per-answer deadlines and earliest deadline; integrate host-provided waking with due-event coalescing, clock corrections, unavailable timers, and recovery.
- **Design keyed listener and answer resources.** Specify singular names, verbs, metadata, lifetime and access checks, and align resource and Rust contracts without reissuing reads.
- **Integrate session push transport.** Bind one subscription to each session's exact read capability, provide bounded SSE or WebSocket delivery, and test disconnect, backpressure, reconnect and full refresh.
- **Specify early cutoff.** Address #613 separately: define when two recomputed representations are equivalent and whether suppressing notifications is safe across authority and context boundaries.
