# Events, streams and resources: one model, and the bridges to the event world

**Status:** design spike, 2026-11-23 (ledger [#4110](http://localhost:1062/l/default/item/4110)). Nothing here
is built and no code changes with this note. Every claim about today's behavior cites the code or the
document it was read from. The revisions read were: ikigai-core main `1fc1ab4`, ikigai-cli `84118c5`,
ikigai-engine `5d4cecf`, ikigai-layers `6971b7e`, ikigai-ledger `d4731f8`, ikigai-name `2011507`,
ikigai-gonk-starter `b3f6245`, and ikigai-gonk's mirror as checked out on 2026-11-23. Three open core
proposals are built on as proposals only, because they are not merged: push for delivered answers
(ikigai-rs/ikigai-core PR 341, `docs/design/push.md`), the action consistency study (PR 335,
`docs/design/action-consistency.md`) and Source negotiation (PR 344).

**The question.** Event-driven systems (logs and brokers, CloudEvents, change data capture, publish and
subscribe, sagas, the outbox) and ikigai's resource model (named resources, representations, golden threads,
caching, capabilities, spaces) describe the same world from two ends. An event is a fact about a change. A
resource is the state that changes produce. A stream is a representation that unfolds over time. This note
says how the three unify in ikigai, where they really differ, how ikigai meets the event world in both
directions (as a SOURCE of resources, events and streams for others, and as a SINK for theirs), and turns
that into guidance a module author or an operator can follow.

**Where this sits.** The mechanisms proposed here belong to the computational and data-processing fabric:
the kernel and its stream bodies, layers, the store, feeds, scripts and the wasm runtime. The work ledger and
its host stay focused: their part is to publish their own transitions for others to consume, to carry a
claim's fence into outside effects, and to stop being polled (section 8). None of them becomes a broker.

---

## 1. The model in five lines

1. **State is a resource; a fact about a change is an immutable resource; a stream is a body.** An event that
   matters is an ENTRY: an atom with its own name that never changes, so it caches forever. A log is the set of
   those entries, read in order by a cursor. A stream is a representation whose bytes arrive over time
   (`Body::Once`), and a stream that must be read again is spilled to a body that can be (`Body::Replay`).
2. **A golden-thread cut is not an event in the event-driven sense. It is an invalidation HINT**: "something
   this answer rests on may have changed". It carries no payload, can over-report, is bounded and lossy (with
   the loss counted), and lives only as long as the process.
3. **Consume edge-triggered, decide level-triggered.** A cut, a feed wake or a webhook says WHEN to look; the
   consumer then reads the state and acts on the difference. Nothing in ikigai promises exactly-once
   delivery. Effects are made safe instead, by idempotency keys and fencing epochs.
4. **Ordering, replay and durability belong to the log that owns the entries**, never to the transport. A
   layer feed is gapless per layer; a cut sequence is ordered per process; nothing orders across them.
5. **A bridge maps names, not semantics.** A foreign event becomes a keyed Sink (its source and id are the
   idempotency key), and an ikigai entry becomes a foreign event (its IRI is the event id, its feed position
   the offset). Authority never travels with the event: each side's door decides who the sender is.

## 2. The seeds, tested against the code

### 2.1 "A golden-thread cut is an event without a payload": qualified

A cut is an event in the narrow sense that it happened at a point in a single order: every `CutEvent` has a
`sequence`, "the cut's place in the kernel's single cut order" (`crates/ikigai-core/src/listen.rs`, the
`CutEvent` struct). It is not a fact that something changed:

- **It over-reports.** A mutating verb cuts its target even when the write ran and failed ("a mutating verb
  still cuts its target, as any write that ran and failed does", `kernel.rs`, `unoffered`, ledger [#1105](http://localhost:1062/l/default/item/1105)). A
  write of the same value cuts, and every dependent recomputes: early cutoff is not built (ledger
  [#613](http://localhost:1062/l/default/item/613)).
- **It under-names.** `CutEvent::invalidated` lists only cached entries, "a result the kernel never stored is
  not listed, because the kernel does not know it was derived from the thread" (`listen.rs`). A non-root
  listener hears a thread only when an entry computed under its own capability depends on it.
- **It is lossy, and says so.** A listener's queue is bounded (`LISTEN_CAPACITY`, 256); a cut that finds it full
  is counted in `CutBatch::dropped` "and must resynchronize" (`listen.rs`, module documentation).
- **It is process-local.** The sequence is not persisted and carries no incarnation, so a listener cannot
  compare sequences across a restart. A listener hears only cuts that begin after `Kernel::listen` returns
  (`listen.rs`, `interested`).
- **Time is not a cut.** An answer that expires is recomputed with no cut at all; core's
  `tests/composite_revalidation.rs` pins that the cut listener stays empty while expiry recomputes. PR 341
  proposes deadline events for that reason.
- **It does not cross a mount.** A mounted read is uncacheable on the mounting side because "nothing crosses
  back to tell this kernel" (`ikigai-engine/crates/ikigai-resolve/src/lib.rs`, `as_held_here`), and that
  comment names a wire call that streams the peer's cuts as "the shape", not built.

**Verdict.** Treat a cut as "look again", never as "this changed to that". It is the right trigger for a
cache, a page, a reconciler and a waiting claimant; forwarded to another system as a domain event, it would
lie (a failed write would announce a change).

### 2.2 "A log or a feed is a resource whose representation grows, with range reads as its replay": holds, with one gap

The layer feed is exactly this. "`seq` starts at 1 and every event is the previous one plus one, assigned
under the layer's write lock … a gap is a defect rather than a quiet hour" (`ikigai-layers/src/feed.rs`,
module documentation). Its read is `source urn:layer:{layer}:feed after={seq} wait={s}`, and `after=0`
replays from the start (ikigai-layers README, "The resources"). Every entry is its own immutable name
(`urn:layer:{layer}:entry:{id}`), so entries cache forever and only the listing moves.

- **The gap: the feed has no incarnation.** A door restored from a backup reuses sequence numbers, and a
  follower whose cursor is at or below the restored head cannot tell (the layer consumer delivery study,
  ledger [#3559](http://localhost:1062/l/default/item/3559), case LF-5, reproduced against a scratch door on 2026-11-21). The fix is an epoch in the feed answer and the
  cursor (section 9, item 3).
- **ikigai-log is a log, but not a stream.** Its segments are hash-chained and replayable as RDF, and are an
  AUDIT record. It has no follow, no index and no central collection (field guide, ikigai-log entry). Do not
  make it the event bus; publish domain events in a feed and let the log record that they were published.

### 2.3 "A subscription is a push of cuts or of new entries": holds, in two tiers that must not be confused

| tier | what is pushed | durable | where it exists |
|---|---|---|---|
| cut listener | invalidation hints, under the listener's own capability | no | core `Kernel::listen` (in process); the HTTP door's `text/event-stream` route (`ikigai-cli/crates/ikigai-web/src/push.rs`) |
| feed follower | new entries, by position | yes | the layer feed's long poll, `wait=` up to 60 s, and one wait over several feeds (`urn:layers:feed after=talk:12,notes:3`) |

The QUIC and IPC wire has no subscription at all: its calls are `Issue`, `IsCached`, `Entries`, `IssueAs`,
`IssueTraced` and `IssueWith` (`ikigai-cli/crates/ikigai-wire/src/lib.rs`, `enum Call`). A client on the wire
that wants to know when a resource changes polls it, and that is where the work ledger's load comes from
(section 8).

### 2.4 "A consumer's offset is a cursor resource": holds as a design, not yet as practice

Today's cursors are files a client keeps (`~/.ikigai/layers-client/follow.cursor`). The consumer study shows
the semantics that matter: acknowledge an entry only after its effect is recorded, or a crash between the
cursor write and the effect loses it (case LN-4, fixed by an acknowledgment cursor written after the record).
Model a cursor as an ATOM in the consumer's own space, written after the effect and, where the effect and the
cursor share a store, in the same update. A consumer group is then a set of cursors with a lease on each
partition, which is what a tuplespace take with a lease already provides (section 3).

### 2.5 "An outbox is a Sink whose effects are entries in a channel": holds, and layers already do it

The layer host stores a write first and logs its feed event second; "an append that fails leaves an event
the log OWES", and the owed event is logged before any later one, including after a restart
(`ikigai-layers/src/feed.rs`, ledger [#1218](http://localhost:1062/l/default/item/1218)). That is the transactional outbox inside one host. The
qualification: a Sink that writes its own store and THEN says something on a separate layer is a dual write,
not an outbox. For a store-backed module the outbox entry must be written in the same store update as the
state it describes (section 8 for the ledger).

### 2.6 "Change data capture is a diff between two representations": qualified

A diff of two snapshots (the `urn:rdf:*` union and difference idiom) gives the NET change. It loses every
intermediate transition, its order and its author: an item claimed, released and claimed again between two
snapshots diffs to "claimed". That is right for synchronizing state and wrong for anything that reacts to
transitions or audits them. True change capture is the owning module's own transition log (2.5), and inbound
change capture from a database (a write-ahead-log reader) is a feed with an offset, consumed under 2.4.

### 2.7 "Event sourcing is a resource composed by folding a log": holds, and is the strongest unification

Because entries never change, **the state after entry n is a pure function of an immutable prefix**, so
`…:state:seq:{n}` can be answered `Expiry::Never`, and `…:state` (the head) is the one moving name, hung from
the feed's append thread. A view as of any position is a temporal space over the log, which is the as-of
corridor the space design already wants. Three cautions:

- A naive fold recomputes from entry 1 after every append. Fold incrementally: `state:seq:{n}` is
  `apply(state:seq:{n-1}, entry:{n})`, with periodic checkpoints (a spill, `urn:stream:spill`) so a cold read
  does not recurse through the whole history. A request's resolution depth is bounded, so a recursive
  composition needs checkpoints anyway.
- The event schema is the contract. A closed SHACL shape per entry type, as layers use, is what lets a fold be
  trusted across versions.
- A fold is a composition, so it is code only where a generic composer cannot do it. Write the first one as a
  stored script (`ikigai-script`) or a SPARQL CONSTRUCT over the entries; promote a generic fold composer only
  when two folds need one.

### 2.8 Two patterns the code already uses, added as seeds

- **The reconciler.** gonk's mirror of the ledger into a task layer is "level-triggered and idempotent: every
  pass reads what IS … and acts on the difference from what SHOULD be"; a cut listener kicks a pass, and the
  feed and a periodic sweep remain as fallbacks when no listener can be had (`ikigai-gonk/src/mirror.rs`,
  module documentation and `spawn_listener`). This is the consumer shape to copy.
- **The webhook as a notification.** ikigai-name refreshes a git-backed namespace when a signed push webhook
  arrives, but "a signed POST only ever causes a read, which is validated like any other"; a forged webhook
  can make the host look, never make it believe (ikigai-name README, "A namespace served from a git
repository"). The payload is
  not trusted; the pull is the data.

## 3. Delivery semantics today

| mechanism | durable | order | delivery | loss is signaled by | backpressure |
|---|---|---|---|---|---|
| cut listener (`Kernel::listen`) | no | kernel cut order, per process | at most once | `dropped` count; full resync | bounded queue (256), the host drains |
| HTTP push (`push.rs`) | no | as the listener | at most once | reconnect means refresh everything shown | the connection; a slow page is closed |
| layer feed | yes, synced | gapless `seq` per layer | at least once to a follower that acknowledges after recording | exit with Conflict when the cursor is past the head; no epoch yet | pull: the follower sets the pace |
| tuplespace take | yes | `seq` per layer | exactly one winner per pending attempt; a lease that lapses returns the tuple and counts the attempt | Conflict to the loser, naming the winner; a late settle is refused | pull |
| ledger claim | yes (store) | per item, `ledger:claimEpoch` | one current holder per epoch | `Conflict` naming the current holder and epoch | none needed |
| streamed body (`Body::Once`) | no | byte order | once, never cached | `Abort` frame or no final chunk: never a clean short body | QUIC flow control; chunk frames; the run's byte budget |
| ikigai-log | yes, hash-chained | per segment | audit only | a broken chain | none: not a delivery path |

## 4. Guidance: "when you have X, model it as Y"

| workflow | model it in ikigai as | what you get for free | what you give up |
|---|---|---|---|
| request and response | a Source (or a Sink) on a name | caching keyed by capability, golden threads, a declared contract, provenance per request | nothing: this is the native case |
| polling | `Expiry::At` from the origin's max-age, plus conditional revalidation (`Representation::revalidated` turns an upstream 304 into the stored body) | one upstream read per deadline for every caller; a 304 costs no body | freshness inside the deadline; and today a revalidated composite still recomputes (ledger [#3772](http://localhost:1062/l/default/item/3772)) |
| push (a page that shows answers) | a cut listener under the page's own capability, behind the door's event stream; or PR 341's delivered-answer subscription | invalidation scoped to what the page read, nothing more disclosed | content: a push says "re-read", never "here is the value"; reconnect means a full refresh |
| subscription to new facts | a feed (a channel layer, or a module's own transition set, 8.1) followed by cursor with `wait=` | durable replay, a gapless order, attribution stamped by the host | latency of one long poll; one host serializes the writes to a layer |
| log replay | `after=0` on the feed; entries read by name | entries cached forever; a late joiner replays without a second protocol | a layer's sequence is per layer, never global |
| change data capture, outbound | the module's own transition feed, written in the same update as the state (2.5) | intermediate transitions with order and author | a module must decide to publish; a snapshot diff is the fallback and loses transitions |
| change data capture, inbound | a connector process that follows the source's log and Sinks each change with `idempotency-key` set from (source, position) | replay safety, attribution to the connector's principal | ordering across partitions; the connector is a separate process with network authority |
| event sourcing | entries as atoms, `state:seq:{n}` as an immutable fold, `state` as the head (2.7) | as-of views for free, cache forever per position | an incremental fold and checkpoints are the author's work |
| outbox | the store-then-log pattern of the layer feed, or the transition written in the same store update | no dual-write window | the producer must own the log; a separate layer is not an outbox |
| saga | a script run (`ikigai-script` `…:runs`) whose steps are Sinks keyed by (run, step), each compensated by another keyed Sink; steps offered as tuples with leases when several workers share them | resend safety, a lapse returns an abandoned step, every step attributed | there is no saga engine and none is proposed; compensation is the author's design |
| batch and ETL | a composition over an as-of snapshot, writing its output with keyed Sinks | rerun only when an input's thread is cut; an unchanged input is a cache hit | a batch over uncacheable inputs recomputes every time, and says why (`urn:kernel:uncached`) |
| streaming bodies | `Body::Once` from a producer, ended by the run's deadline; `urn:stream:spill` when it must be read again; `urn:stream:range` over a seekable body | bounds that refuse and never truncate, typed aborts, byte budgets riding the capability | a one-shot body is never cached; a live tail is a feed, not a body |
| feeds (RSS, Atom) | ikigai-feed lifts them into RDF; a feed layer publishes them out, filtered by SKOS concept | one lift for every filter, mapping and view; one shared cached copy for all readers | inbound feeds are polled by nature; WebSub is not built |

Three rules fall out of the table:

- **Never put a mutable or uncacheable source into an expensive cached composite to make it "live".** Liveness
  comes from a listener or a feed; the composite stays cacheable and is invalidated, not polled.
- **Never send a cut to another system as a fact.** Publish an entry.
- **Make every consumer's effect idempotent before you worry about delivery guarantees.** Core's idempotency
  toolkit (`crates/ikigai-core/src/idempotency.rs`, `idempotency-key=` with `Resend::Key`) is the shared home.

## 5. The source and sink matrix

ikigai as a SOURCE sends resources, events and streams out; as a SINK it takes them in. Each row is split by
whether the other side is another ikigai kernel (a mount over the wire, or a layer between two organizations)
or a foreign system.

### 5.1 Resources

| direction | peer | model | exists | missing |
|---|---|---|---|---|
| out | ikigai | a mount: the peer resolves names over QUIC or IPC | mounts, transports, capability-on-the-wire (`IssueAs`), the run context (`IssueWith`) | mounted reads are uncacheable on the mounting side until a peer's cuts cross (item 2); immutable answers are the exception being designed (`immutable-mounted-answers.md`, ledger [#2686](http://localhost:1062/l/default/item/2686)) |
| out | foreign | the HTTP door: GET with `ETag`, `304`, `no-cache` for answers that are cacheable but threaded | the door, validators, `Accept` negotiation (PR 344 proposes Source negotiation) | nothing structural; `Range` is designed in the streams work |
| in | ikigai | mount the peer and compose over its names | same as above | same as above |
| in | foreign | `urn:http:get` under `urn:cap:net:*`, cached by the origin's headers and revalidated | ikigai-http, revalidation | a revalidated composite still recomputes (ledger [#3772](http://localhost:1062/l/default/item/3772)) |

### 5.2 Events

| direction | peer | model | exists | missing |
|---|---|---|---|---|
| out | ikigai | a layer feed the peer follows with its own grant; a cut stream for invalidation | layer feeds, multi-feed waits, open join for public feeds | an epoch in the feed (item 3); cuts across mounts (item 2) |
| out | foreign | a delivery relay that follows a feed and delivers each entry as a CloudEvent: a webhook POST, a broker publish, an SSE stream with `id` set to the cursor | the HTTP door's event stream carries cuts only | the relay (item 7), the CloudEvents mapping (item 6), a feed face over SSE with `Last-Event-ID` (item 7) |
| in | ikigai | follow the peer's feed; a gonk-to-layer mirror is the worked example | layers clients, gonk's mirror | the follower's cursor as a resource; epoch checks |
| in | foreign | a door route that verifies the sender and either (a) refreshes (notification, then pull) or (b) Sinks the event with `idempotency-key` derived from (source, id) into a channel layer or a module | ikigai-name's signed-webhook refresh; core's idempotency toolkit | a generic intake route and the CloudEvents lift (items 6, 8) |

### 5.3 Streams

| direction | peer | model | exists | missing |
|---|---|---|---|---|
| out | ikigai | a `Body::Once` or `Replay` answer framed in wire v10 chunks | core bodies, the stream builtins, the wire's body frames | the faces in Python, TypeScript and .NET stay on the collected form until they add the frames |
| out | foreign | the HTTP door with `Content-Length` or chunked encoding; NDJSON for a sequence of values | the design (the binary streams arc, ledger [#2228](http://localhost:1062/l/default/item/2228)) | a layer feed as an open-ended NDJSON body (ledger [#2283](http://localhost:1062/l/default/item/2283)) |
| in | ikigai | a Sink whose content arrives by `ArgRef::Content` after a spill | the content store and spill seams in core | uploads through every door |
| in | foreign | spill, then dispatch: the upload is hashed while written and the Sink runs on its digest | the design; HTTP intake refuses chunked bodies on purpose | an upload route with a per-route bound and a minimum rate |

### 5.4 What carries across the boundary

| property | out (ikigai as source) | in (ikigai as sink) |
|---|---|---|
| capabilities | a listener or a relay acts under the capability of the reader it serves and discloses only what that capability read; a relay subscription is a narrowed grant and is rechecked at each delivery, so revocation stops delivery | the door authenticates the sender (client certificate, passkey, a webhook signature) and mints the principal; the event's own `source` field is a claim, not an identity; the intake route holds only the narrow Sink grant it needs |
| golden threads | do not cross today (`Representation::threads` is not serialized); what crosses is names and positions | a foreign change cuts the thread of the atom its Sink writes, and everything derived recomputes |
| provenance | entries carry `prov:wasAttributedTo`, `dcterms:created` and `ik:seq`, stamped by the host; on the way out they map to CloudEvents attributes (6.1); trace context maps to `traceparent` | the intake records the door's principal as the author and the event's own `time` as an AUTHORED time, never as commit order (the ledger's clock research, ikigai-rs/ikigai-ledger PR 134) |
| backpressure | pull-based feeds pace themselves; a push channel is bounded and closes a slow consumer, which must resynchronize; streams use QUIC flow control and the run's byte budget | a door bound per route (size, rate, concurrency) and a refusal with `Retry-After`; the intake never buffers without a bound |

## 6. Bridges to the event world

Every bridge lives OUTSIDE core. Core supplies the hint (cuts), the body (streams), the idempotency toolkit and
the capability model; a library or a connector process does the mapping. A bridge that opens sockets to a
broker is a connector process with its own network grant, mounted by a host, never a library linked into a
module (field guide, Hypothesis H and the ambient-authority check).

### 6.1 CloudEvents, both directions

The mapping, one implementation with vectors that the Python, TypeScript and .NET faces pin:

| CloudEvents attribute | ikigai |
|---|---|
| `id` | the entry IRI (`urn:layer:{layer}:entry:{id}`), unique within its source |
| `source` | the feed's name (`urn:layer:{layer}`, or a module's transition set) |
| `type` | the entry's `rdf:type` as an IRI, or the feed event's kind (`say`, `out`, `taken`, `returned`, `sweep`) |
| `subject` | the resource the event is about (a tuple subject, an item IRI) |
| `time` | `dcterms:created`, the host clock's stamp |
| `datacontenttype` | `text/turtle` or `application/ld+json` (the layer's egress context) |
| extensions | `ikseq` (the feed position) and `ikepoch` (the feed's incarnation, item 3); `traceparent` from the run's trace context |

- **Out.** A view over a feed range answers `application/cloudevents-batch+json`, and one entry answers
  `application/cloudevents+json`, through the transreptor pair below. A relay posts them in structured mode, or
  in binary mode with `ce-` headers.
- **In.** A CloudEvent is lifted to Turtle and written with a keyed Sink whose `idempotency-key` is a digest of
  (`source`, `id`), so a redelivery answers the first answer and writes nothing. The event's `time` is kept as
  authored time. A CloudEvent that names an `ikseq` from a peer is advisory: only the receiving log assigns
  positions.
- **Home.** ikigai-feed (in the ikigai-linkeddata workspace) already lifts RSS and Atom into RDF and mints no
  terms of its own; CloudEvents is the same kind of lift. Putting the pair there keeps one home for "foreign
  event and feed formats as RDF", which the standards rule asks for.

### 6.2 Log and broker systems (Kafka-style)

- **Out:** a relay produces each feed entry to a topic with the entry IRI as the record key and the CloudEvents
  binary mapping in headers. Order is preserved per layer when one layer maps to one partition. The relay's
  cursor advances only after the broker acknowledges.
- **In:** a connector consumes a topic and Sinks each record keyed by (topic, partition, offset), committing
  the offset only after the Sink lands. Ordering is per partition; a module that needs a cross-partition order
  must not get it from the broker.
- **Where:** a connector process with a network grant, mounted by a host. No broker client goes into core or
  into a module library.

### 6.3 Webhooks

- **In:** prefer notification, then pull (2.8): the route verifies the signature and triggers a refresh read,
  so the payload is never trusted. When the payload is the only copy of the data, the route Sinks it keyed by
  the delivery id. Signature verification uses the shared HTTP and signature homes (ikigai-http,
  ikigai-sign), never a private HMAC.
- **Out:** the relay posts with the Standard Webhooks headers (`webhook-id` set to the entry IRI's digest,
  `webhook-timestamp`, `webhook-signature`), retries with backoff, and after its limit moves the delivery to a
  dead-letter tuplespace a person can take from. A webhook never carries authority; the receiver's door
  decides.

### 6.4 Server-sent events and WebSocket

- The HTTP door's event stream exists for cuts (`push.rs`). Add a feed face on the same machinery: each event's
  `id` is `{epoch}:{seq}`, and a reconnect with `Last-Event-ID` resumes from it.
- WebSocket only where a page needs to send as well as receive; ikigai peers use QUIC or WebTransport, which
  already carry the wire's frames.

### 6.5 Change data capture sources

- A database's change stream (a write-ahead-log reader, or a Debezium-style envelope with `before`, `after`
  and a source position) is consumed by a connector as in 6.2. The snapshot-then-stream handoff is the
  connector's job and must record the position where the snapshot was taken, so no change between the two is
  lost.
- Into ikigai-store, each change is a keyed graph update. The store cuts per graph only once per-graph threads
  exist (very-large-graphs direction); until then one write cuts every cached read of the store.

### 6.6 Where each bridge lives

| bridge | owner | kind |
|---|---|---|
| CloudEvents lift and lower | ikigai-feed (ikigai-linkeddata) | transreptor pair, with vectors |
| feed over SSE | ikigai-cli (`ikigai-web`, beside `push.rs`) | door view |
| inbound webhook and CloudEvents intake | ikigai-cli (`ikigai-web` route), verification through ikigai-http and ikigai-sign | door route |
| delivery relay (webhook out, broker out, dead letters) | a new module, `ikigai-relay` (item 7) | host library plus atoms |
| broker and database connectors | separate connector processes, one per system | mounted process |

## 7. Fencing: carrying a lease epoch into effects outside ikigai

Inside ikigai the fence exists twice. A ledger claim carries `ledger:claimEpoch`, raised by every grant and
takeover, and a holder's write with `epoch=` lands only while that epoch is current, checked "inside the
write's own store update" (ikigai-ledger README, "A claim carries a fencing epoch"). A leased tuplespace take
records its attempt, and after a lapse "`done`, `renew` and a report on the offer from the late taker are each
refused as a Conflict" (ikigai-layers README, "Leases").

Outside ikigai, a fence only works where the resource being changed CHECKS it, and remembers the highest
epoch it has seen (the classic fencing-token argument, Kleppmann, "How to do distributed locking", 2016).
A git push, a pull request or an outside API does not check our epoch. So:

1. **Route the effect through a Sink we own when we can.** The Sink checks the epoch (`source
   urn:iki:ledger:claim item=N epoch=E` answers `current` or `superseded`) and performs the effect in one
   step. The check and the effect are not atomic across systems, so this narrows the window; it does not
   close it.
2. **Derive the effect's idempotency key from the fence.** A key such as `{ledger}:{item}:{epoch}:{step}`
   means a stale holder's retry and the current holder's attempt can never be mistaken for one another, and a
   receiver that keeps keys refuses the stale one.
3. **Record the epoch in what the effect leaves behind** (a branch name, a pull request's text, a commit
   trailer), so whoever later accepts the effect (a merge, a close) checks that the epoch is still current
   before accepting it. That moves the fence to the last step, which we do own.
4. **A signed, scoped fencing token is warranted only where an outside verifier will check it offline.** Such
   a token (holder, item, epoch, audience, not-after, signed with ikigai-sign) proves that the epoch was
   granted; it does not by itself give monotonicity, which still needs the verifier to remember the highest
   epoch. Build it when a second ikigai host performs effects for a claim it cannot call back about, not
   before.

Lease time needs a time authority as well as a logical fence: one host's clock grants and lapses, and an
observer on another clock can report a lease expired early or late (the ledger's clock research, ikigai-rs/ikigai-ledger PR 134).

## 8. The work ledger and its host, specifically

The ledger is a module; its host serves it. Neither becomes an event platform.

### 8.1 Transitions as events

The ledger publishes no transitions today. Its history graph orders events by authored time, and
comment order inverts under clock skew (ikigai-rs/ikigai-ledger PR 134 reproduces it). The proposal:

- A per-ledger durable sequence, allocated in the same store update as each transition (the clock research's
  first contract).
- A transition set, `urn:iki:ledger:{ledger}:transition`, whose members `…:transition:seq:{n}` are immutable
  entries (item, kind: filed, claimed, renewed, released, lapsed, handed back, closed, reopened; the epoch;
  the principal; the time). It is written in the same store update as the state it records, which makes it the
  ledger's outbox.
- A feed face: `source urn:iki:ledger:{ledger}:transition after={n} wait={s}`, read under `read:{ledger}`, the
  same shape as a layer feed, so every follower and every relay works with no new client code.

### 8.2 Claims from a topic

This already exists, as composition: gonk's mirror offers open, unclaimed items as tuples in a task layer,
and a take there becomes a claim for the taker (`mirror.rs`, "Then each item"). A worker that follows the
task layer's feed instead of asking `next` is consuming claims from a topic, with the layer's atomic take as
the single winner and the claim's epoch as the fence.

### 8.3 What replaces polling `next`

Two machines asked `urn:iki:ledger:default:next` 144,550 times in 12 days, 58% of the host's authenticated
requests, nearly always getting the same answer (a 12-day study of the host's request log, ledger [#3391](http://localhost:1062/l/default/item/3391), 2026-11-23). `next` is
cached under the store's write threads and is valid until the moment its policy's ranking or the next lease
expiry can change it (ikigai-ledger `src/policy.rs`, `valid_until`; `src/select.rs`, `next_lease_expiry`),
so every ingredient of a wait already exists: a cut for a write, a deadline for time. In order of
preference:

1. **Wait on a change, in general.** A listener resource (PR 341's proposed `urn:kernel:listener:id:{id}`) that
   a client creates under its own capability for the reads it has made, and drains with `source … wait={s}`.
   A long-poll Source works on every transport today, so the wire needs no new call. A worker reads `next`,
   registers, waits, reads again on a wake. This also answers ledger
   [#1144](http://localhost:1062/l/default/item/1144) ("tell the hub when an item changes") with no ledger
   code.
2. **Conditional wait as sugar.** `source urn:iki:ledger:default:next` carrying, in the run context, the
   validator of the answer the caller holds and a wait: the door answers when the answer's threads are cut or
   its deadline passes, and "not modified" otherwise. The run context (`IssueWith`) is where the deadline
   already rides, because these options change how long the caller waits, not what the answer is.
3. **Follow the task layer or the transition feed** (8.1, 8.2), for workers that want offers rather than a
   ranking.

The ledger's README already documents the in-process version of 1 for a held claim (ledger [#3711](http://localhost:1062/l/default/item/3711)): wait on
the store's threads, re-read on a wake, and never treat a wake as proof that the item is free.

## 9. Follow-up items (for the hub to file)

Each names its owner in the fabric. None is in the work ledger's host except where it serves the ledger.

1. **core and ikigai-cli: wait for a change on any resource.** Build PR 341's listener resource
   (`urn:kernel:listener:id:{id}`) with a long-poll drain (`wait=`), scoped to the creating capability and
   bounded, and expose it through every door. Then the conditional form: a validator and a wait in the run
   context, answered on a cut, a deadline or a timeout. Measure against the 58% `next` load; the acceptance
   test is a worker that makes one request per change instead of one per minute.
2. **ikigai-engine and ikigai-cli: a peer's cuts cross a mount.** The mounting kernel subscribes, on the peer,
   to the reads it has made (a target-matched subscription, PR 341, so thread names need not cross), and cuts
   its own copy of the mounted name on each wake. Mounted mutable answers then become cacheable here and
   fresh. A dropped or broken subscription falls back to today's uncacheable rule, and says so through
   `urn:kernel:uncached`.
3. **ikigai-layers: a feed epoch.** The door names its feed's incarnation; the feed answer and the cursor
   carry it, and a cursor from another epoch is refused as Conflict the way a cursor past the head already is.
   Closes the restored-door gap (LF-5) and gives CloudEvents and SSE ids something stable to carry.
4. **core: a kernel incarnation on cut events.** A listener's `sequence` is per process; an incarnation id on
   `CutEvent` and `CutBatch` lets a consumer that outlives a kernel restart know it must resynchronize. Small,
   additive (`CutEvent` is `#[non_exhaustive]`).
5. **ikigai-ledger: a transition feed as the ledger's outbox.** The durable per-ledger sequence, the
   transition set written in the same store update, and the feed face (8.1). Includes the clock research's
   separation of authored time from commit order.
6. **ikigai-feed: CloudEvents lift and lower.** A transreptor pair between `application/cloudevents+json`
   (and the batch form) and Turtle, with the attribute mapping of 6.1 and vectors the faces pin.
7. **A new module, `ikigai-relay`: delivery out of ikigai.** Subscriptions as atoms
   (`urn:relay:subscription:id:{id}`: what to follow, a filter, a destination, a narrowed grant), a cursor atom
   per subscription written after the destination acknowledges, delivery records, a dead-letter tuplespace,
   and destinations for webhooks (Standard Webhooks signatures) and for an SSE face. Broker destinations are
   connector processes it calls, not libraries it links.
8. **ikigai-cli (`ikigai-web`): an intake route for webhooks and CloudEvents.** Verify the sender through the
   shared HTTP and signature homes; then either refresh (notification, then pull) or Sink with an idempotency
   key from (source, id). Size, rate and concurrency bounds per route.
9. **A fold, as a worked example first.** An event-sourced resource in The ikigai Book (or the tutorial
   crates): entries as atoms, `state:seq:{n}` cached forever, checkpoints by spill, the head as the one moving
   name. A generic fold composer (ikigai-fn or ikigai-script) only after two real folds need one.
10. **ikigai-script: triggers.** Run a published script when a feed advances or a listener wakes, under the
    script's own authority rules. The README lists triggers as a later phase; this note gives them their
    inputs (items 1, 3, 5) and keeps them out of core.
11. **ikigai-runtime: stream values as bodies.** Project a component's `stream<T>` as an NDJSON body and feed a
    `stream<u8>` parameter from a body (the binary streams design already decided this), so a wasm consumer or
    producer of events is an ordinary endpoint.
12. **A convention, not a feature: fences in outside effects** (section 7). Document epoch-derived keys and
    epoch-carrying artifacts in the ledger's README and the worker's guide; defer a signed fencing token until
    a cross-host effect needs offline verification.

## 10. What not to build

- **A broker in core, or in the work ledger's host.** Logs belong to the modules that own their facts; the
  kernel's part is the hint and the body.
- **Exactly-once delivery.** Promise at-least-once plus idempotent effects, and say so.
- **Cuts with payloads, or a durable cut log.** A cut is a hint; making it durable would make a lossy
  over-approximation look like a fact.
- **A saga or workflow engine in core.** Scripts, keyed Sinks and leased tuples compose one.
- **A broker client linked into a module library.** Connectors are processes with their own network grant.
- **Authority carried by an event.** A signature authenticates where a message came from; the receiving
  door's grant decides what it may do.
- **ikigai-log as an event bus.** It records what happened, including what was published; it does not
  deliver.
- **A range family per module.** Replay is `after=` on a feed and `urn:stream:range` on a body, written once.

## 11. Resource map

Names are singular, the type first, and every partition is by a named key. Proposed names are for review.

| set | name | kind | how |
|---|---|---|---|
| a layer entry | `urn:layer:{layer}:entry:{id}` | **ATOM** (exists) | immutable; cached forever |
| a layer feed | `urn:layer:{layer}:feed` | composition (exists) | `after=`, `wait=`; gains an epoch (item 3) |
| a ledger transition | `urn:iki:ledger:{ledger}:transition:seq:{n}` | **ATOM** | written in the same store update as the state it records |
| the transition feed | `urn:iki:ledger:{ledger}:transition` | composition | `after=`, `wait=`, as a layer feed |
| a listener | `urn:kernel:listener:id:{id}` | **ATOM** (session state, PR 341) | Sink creates; Source drains with `wait=`; Delete ends it |
| an event-sourced state at a position | `urn:{ns}:state:seq:{n}` | composition | fold of an immutable prefix; `Expiry::Never` |
| the current state | `urn:{ns}:state` | composition | the fold at the head; hung from the feed's append thread |
| a checkpoint | `urn:{ns}:checkpoint:seq:{n}` | spill (`urn:stream:spill`) | so a cold read does not recurse through history |
| a CloudEvent face | `… as=application/cloudevents+json` or `…-batch+json` | transreptor | ikigai-feed (item 6) |
| a relay subscription | `urn:relay:subscription:id:{id}` | **ATOM** | what to follow, a filter, a destination, a narrowed grant |
| a relay cursor | `urn:relay:cursor:subscription:{id}` | **ATOM** | written after the destination acknowledges |
| a delivery | `urn:relay:delivery:subscription:{id}:seq:{n}` | **ATOM** | attempts and outcome |
| dead letters | a tuplespace layer per relay | **ATOM** (a layer) | a person takes and retries or drops |
| a relay's status | `urn:relay:status:subscription:{id}` | composition | lag, last delivery, failures; uncacheable by design |
| an intake | a door route | door view | verifies, then refreshes or Sinks with a key |

**Spaces.** A cursor lives in its consumer's own space, layered over the shared feed. A view at a feed position
is a temporal space over the log. A relay subscription runs in a corridor narrowed to its grant. The
`urn:kernel:*` names stay sealed.

**Code that cannot be a composition, one line each.** The listener's queue and the wait (kernel state on the
writer's path). The feed epoch (the door's identity of its own state). The transition sequence (a counter
allocated inside a store transaction). The CloudEvents transreptor (a format mapping, written once with
vectors). A relay's destination clients (the I/O itself). Every view, filter, fold and status above is a
composition.
