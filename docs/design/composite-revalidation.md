# Revalidating an expired composite through its dependencies — design

**Status:** design only (ledger #3772, found by #2753). Nothing here is built. What the kernel
does today is pinned by `crates/ikigai-core/tests/composite_revalidation.rs`. The first
increment (below) flips that file's last count from `[2, 2, 2, 2, 2]` to `[1, 1, 1, 1, 2]` and
un-ignores `unchanged_revalidation_reuses_feed_transforms` in ikigai-http.

**Anchor:** core 0.3.5, `crates/ikigai-core/src/kernel.rs` (the cache lookup, expiry
propagation, `Revalidate`), `cache.rs` (`serve_or_offer`, `state_of`, `Offer`), `endpoint.rs`
(`Recorded::record`), `repr.rs` (`Representation::revalidated`, `content_id`).
**Companions:** `docs/design/cache-ejection.md` (what an entry's validity is made of),
`docs/formalism/README.md` R4.1 (validity) and the expiry meet below it.

---

## The gap, reproduced

ikigai-http's ignored test `unchanged_revalidation_reuses_feed_transforms` (ikigai-http PR 47,
merged 2026-11-15) runs five feed transforms over five HTTP feeds served with `max-age=1`. At the
deadline four origins answer 304 Not Modified, which ikigai-http turns into
`Representation::revalidated`, and one answers a changed 200. Every transform runs again: the
counts go from `[1, 1, 1, 1, 1]` to `[2, 2, 2, 2, 2]`, not `[1, 1, 1, 1, 2]`. A root cut listener
sees no cuts.

`tests/composite_revalidation.rs` reproduces this in core with no HTTP:
`unchanged_revalidation_still_reruns_every_feed_transform` binds five feeds (a conditional GET
in miniature that answers `revalidated` when the offered entity tag is still current) and five
transforms that each read one feed. Past the deadline, the feeds' bodies are produced
`[1, 1, 1, 1, 2]` times, which is right. The transforms run `[2, 2, 2, 2, 2]` times, which is the
defect. The listener's queue is empty.

### Why, in core's code

1. **A composite's deadline is its parts' deadline.** After the endpoint answers, the kernel
   meets the endpoint's own expiry with `invocation.dependency_expiry()` (`kernel.rs`, "Effective
   expiry propagates from the dependencies", about line 4992). `Recorded::record`
   (`endpoint.rs`, line 679) keeps only that meet and the union of the parts' threads. It does not
   keep which requests were read or what they answered. A transform that answered
   `.cacheable()` (`Never`) over a feed valid until 1100 is stored until 1100.
2. **At the deadline, the lookup evicts it.** `ReprCache::serve_or_offer` (`cache.rs`, line 1332)
   asks `state_of` (line 1902). Edges are checked first, so an entry with a cut edge is `Cut`. An
   entry whose edges stand but whose deadline has passed is `Expired`. An expired entry is kept
   and offered (`Lookup::Offer`) only when it carries an origin validator. A composite has none,
   since it has no origin. So it falls through to the arm that removes it, and the lookup is a
   miss.
3. **So the endpoint is invoked before anyone learns the part is unchanged.** On a miss the
   issue path invokes the transform (`kernel.rs`, `invoke_body`, about line 4911). The
   transform's own `inv.source(feed)` is what reaches the feed's expired entry. That entry has a
   validator, so it is offered, wrapped in `Revalidate` (`settle_references`, line 971;
   `struct Revalidate`, line 6846), and the 304 becomes the stored body under the new deadline.
   That refresh is correct, but it happens inside the transform's run, after the decision to
   run the transform has been made. Nothing the feed answers can undo that decision.

No thread is cut anywhere in this path. The cut machinery is not involved, and widening it would
not help.

## The idea

Keep an expired composite instead of evicting it, along with a **record of what it read**. When
its deadline passes, first re-issue each recorded read. If every one comes back unchanged, serve
the stored composite under the new deadlines without invoking its endpoint. If any one changed,
discard the entry and invoke the endpoint as today. Its reads are now cache hits, because the
round just refreshed them.

This is the HTTP 304 idea one level up. The composite's "validator" is the list of its inputs'
content identities.

## What core records per cached composite

A **dependency record**, kept with the entry (beside `Slot::edges`). It lists one item per
successful cacheable sub-request the invocation made, in the order they completed:

| Field | Why |
|---|---|
| the `Request` as issued (verb, target, arguments) | to issue it again exactly |
| the capability it was issued under (a shared handle) | the partition it was read in; see below |
| the scope it resolved in (the invocation's `scope.descend(levels)`, shared) | the same corridors, the same endpoint |
| the incoming provenance, when it was piped | part of that read's key |
| its **content identity**: `Representation::content_id()` (blake3 over type and bytes, `repr.rs` line 366) | what "unchanged" is compared on |

Plus, once per entry, the endpoint's **own** expiry before the meet (the value at
`representation.expiry` before `most_restrictive(invocation.dependency_expiry())`). Today that
value is gone once the meet is taken, and the design needs it: see "Freshness bounds".

`Recorded::record` already runs for every sub-request result and already holds a lock on the
dependency record. It gains one push, made only when the issuing request is a cacheable verb on a
kernel with a clock, so the record can ever be used.

**A composite is eligible** for this only when every sub-request it made was a success with a
body that can be read again. Its record is dropped, and the entry behaves as today, when any
sub-request:

- failed or missed (`record_miss`, a caught `NotFound`, a cached failure: ledger #611, #2657),
- answered a one-shot body (`Body::Once`, which is never stored, ledger #2237),
- or pushed the record past a fixed cap (decision 5).

A composite whose own answer carries an origin validator keeps today's offer path (decision 7).

## The revalidation round

In `serve_or_offer`, an `Expired` entry with a dependency record and no origin validator is
kept, not evicted, and returned as a new `Lookup::Recheck(Arc<Recheck>)`. It is the same shape as
`Lookup::Offer`: a second handle on the stored body plus `stored_tick`, so `discard` forgets only
that entry. On the issue path, after `served_from_cache` and before the endpoint is invoked:

1. Take a cut snapshot (`self.cache.snapshot()`, as `taken` is taken for any store).
2. Issue each recorded request again under its recorded capability and scope, as a
   sub-request of this one (same span parent, depth plus one, the run's budgets). Each goes
   through the whole issue path. A part that is fresh is a hit. A part that expired is offered
   to its endpoint (the HTTP 304 path). A part that is itself a composite runs its own round, so
   nesting composes.
3. Compare each answer's `content_id()` to the recorded one. Stop at the first difference.
4. **All unchanged:** build the answer from the stored body. Its expiry is the meet of the
   endpoint's own recorded expiry and every new answer's expiry (and the incoming one, when
   piped). Its threads are the endpoint's own declared threads, the target thread, and the union
   of the new answers' threads. Store it through `store_outcome` with the snapshot from step 1,
   so the lost-cut rule applies exactly as to a computed answer. Trace the step as
   `TraceState::Revalidated` (decision 8).
5. **Any changed, or any failed:** `discard` the entry and fall through to the invocation, which
   records a fresh dependency record.

The round runs out of line (`#[inline(never)]`, a boxed future), as `served_from_cache` and
`Revalidate` do. The issue path's frame grows by one pointer, which every nesting level pays for
(the 65-deep chain test, ledger #2538, #2688).

## What "unchanged" means

**Equal content identity:** the same `ReprType` and the same bytes, by `content_id()`. It is the
only rule, and it covers all three ways a part can come back:

- **A fresh hit.** The bytes are the stored bytes, so the digest matches.
- **The revalidated marker** (`Representation::revalidated`, an HTTP 304). `Revalidate` serves
  the stored body, so the digest matches *when that stored body is the one the composite read*.
  If the part's entry was replaced in between (another reader got a 200), the stored body is
  different and the digest says so. The marker alone is not enough, because it vouches for the
  part's current entry, not for the bytes the composite saw.
- **A new 200 with identical bytes.** The digest matches, so the composite is reused. This also
  serves origins that send no validator at all.

**An ETag is not compared by core.** Core never parses one (`OriginValidator` is opaque,
`repr.rs` line 232). An ETag match reaches core as the revalidated marker, which then reaches
the digest rule above. A weak ETag that matches across byte-different bodies is caught by the
digest too.

Threads and validators are not part of "unchanged". A part that answers the same bytes with
different threads is unchanged, and its new threads are what the reused composite hangs from.

## What it preserves

**Capability partitions.** The composite's entry stays under its `CacheKey`, which includes
`capability_key(capability)` (`kernel.rs` line 4816). A reader under another capability looks up
another key and never reaches this entry or its record. The round re-issues each part under the
capability that part was *read* under (which may be narrower than the composite's, through
`issue_attenuated`), never the current reader's. Each re-issue runs the declared-capability
floor again, so a part that is now refused answers `Denied`, which is "changed". The composite
then runs and is refused or answers as it would today.

**Explicit cuts.** `state_of` checks edges before the deadline. An entry with any cut edge is
`Cut` and is evicted as today. It is never rechecked. A cut that lands *during* the round moves
a generation after the snapshot in step 1, so `store_outcome` declines the store
(`cut-in-flight`). The design does not serve that answer either: it discards and invokes the
endpoint (decision 10). A cut always invalidates at once, as it does today
(`docs/formalism`, ledger #2274).

**Freshness bounds.** A reused answer's deadline is the meet of the new deadlines, so it never
outlives any part's new deadline. It also never outlives the endpoint's *own* deadline. If the
endpoint's own expiry was `At(t)` and `t` has passed, the entry is not eligible, because no
part's refresh can extend what the endpoint said about itself. That is why the record keeps the
own expiry from before the meet. A kernel with no clock never stores an `At` entry, so it never
gets here.

**Conditional reads.** A composite that reads different parts on different runs (it reads
`flag` and then `a` or `b`) is handled by recording the reads *this run* made. If every one of
them is unchanged, the endpoint's inputs are identical to last time, so a run would take the
same branch and read the same parts. If `flag` changed, the round stops there and the endpoint
runs, takes the other branch, and records `flag, b`. This rests on the assumption core already
makes to cache a composite at all: the answer is a function of the request and what the
endpoint read through the kernel (Hyp. H, `docs/formalism/README.md`). An endpoint that reads
the clock or other ambient state is already `Always` (or must say so) and is never stored. A
read whose *request* depends on an earlier read's bytes is also fine. If the earlier read is
unchanged, the later request is the same request.

## What it costs

**Memory.** For each eligible entry, one record item per part: the request (its target IRI and
arguments, typically 50 to 200 bytes), two shared handles (capability, scope: a pointer each), an
optional provenance, and a 32-byte digest. That comes to roughly 100 to 300 bytes per part. Five
feeds cost about a kilobyte in all. A composite over ten thousand reads would cost megabytes,
which is why there is a cap (decision 5). An expired entry is also held past its deadline until
a read rechecks it or the policy evicts it, as an offered entry already is (ledger #2534). The
record is metadata. `CacheEntry::bytes` counts payload only, so the record is reported beside it
(decision 9).

**Time, on every eligible store:** one blake3 per part, over bytes the composite already holds.
blake3 runs at gigabytes per second, so this is small next to an XSLT lift, but it is paid by
every cacheable composite whether or not it ever expires (decision 3).

**Time, at expiry:** one issue per part. These are the same reads a rerun would make anyway, so
when every part is unchanged the round costs a rerun's reads and saves the endpoint's own work.
When one part changed, the endpoint runs as today, and its reads are now cache hits. The extra
cost is one lookup per part already checked, a few hundred nanoseconds each on a hit (formalism
§10). For HTTP parts, the round's latency is the conditional GETs, sequential or concurrent
(decision 4).

## Tests that would prove it

1. **The reproduction.** `composite_revalidation.rs`: the transform counts flip to
   `[1, 1, 1, 1, 2]`, the reused answers carry the new deadline, and the listener stays empty.
   Then ikigai-http's `unchanged_revalidation_reuses_feed_transforms` is un-ignored and passes
   against the released core.
2. **Freshness:** two parts revalidated to +50 and +200. The reused composite expires at +50.
   The endpoint's own `At` passed means it runs, even when every part is unchanged.
3. **A cut wins:** cut a part's thread while the composite is expired. It runs, no recheck
   happens, and the listener names it. A cut landing between the round and the store declines
   the store, and the endpoint runs.
4. **Capability partitions:** a composite cached under capability A is expired, and a reader
   under B computes its own. The recheck under A re-issues under A's recorded capability. A part
   attenuated by the composite is re-issued attenuated. A part now denied means the composite
   runs.
5. **"Unchanged":** a 200 with identical bytes is reused. A 304 against a part entry that was
   replaced since (different bytes) means the composite runs. A same-bytes answer with new
   threads is reused and hangs from the new threads (a later cut of a new thread drops it).
6. **Conditional reads:** flag unchanged means reuse. Flag changed means the composite runs,
   reads the other branch, and the next record names it. The third run rechecks the new branch.
7. **Nesting:** outer over inner over feed, with everything unchanged. Neither composite runs,
   and both are traced `revalidated`.
8. **Ineligible parts:** a caught `NotFound`, a cached failure, a one-shot body, and a record
   over the cap each give today's behavior, pinned by count.
9. **Frame and readouts:** the 65-deep chain test still passes on macOS debug (CI's macOS job).
   `urn:kernel:cache` shows the record's size. The trace shows the composite `revalidated` with
   its rechecked parts as child events.

## Build order

1. Record the own expiry and the dependency record in `Recorded`, with eligibility and the cap.
   Store it with the entry. No lookup change yet. Test: the record's contents, and memory in the
   readout.
2. `Lookup::Recheck` and the sequential round with short-circuit. Flip test 1, and add tests 2
   through 8.
3. Release core. Un-ignore the ikigai-http test there.
4. Measure the HTTP case. Decide on a concurrent round (decision 4).

## Decisions for Brian

1. **Automatic or opt-in?** Rerunning on expiry is today's contract, and reuse needs the
   endpoint to be a function of its reads, which caching already assumes. *Recommendation:*
   automatic for every eligible composite, so the ikigai-http transforms need no change, with an
   endpoint opt-out on the representation for an endpoint that wants to run on every deadline.
2. **What "unchanged" is compared on.** *Recommendation:* content identity (`content_id()`)
   only. Do not trust the revalidated marker or an ETag on its own, because they vouch for the
   part's current entry, not for the bytes the composite read.
3. **Hash at record time, or identify the stored entry?** Comparing `stored_tick` of the part's
   entry would avoid hashing, but it misses an identical 200 and depends on the part being
   stored at all (a policy may decline). *Recommendation:* blake3 at record time, and measure it
   on the cache-hit benchmark (formalism §10) before step 2 ships.
4. **Sequential or concurrent round?** Sequential short-circuits on the first change.
   Concurrent overlaps the HTTP round trips. *Recommendation:* sequential in record order for
   the first increment. Make it concurrent on a scheduled kernel only after step 4 measures the
   HTTP case.
5. **A cap on parts per record.** *Recommendation:* 64 parts, a constant at first and a
   `cache` setting later. Over the cap, there is no record and today's behavior applies. The cache
   readout says so, so the fallback is never silent.
6. **Which parts make a composite ineligible.** *Recommendation:* failures, misses, cached
   failures and one-shot bodies, for the first increment. A miss could later be compared as
   "still missing", but its `carried` dependencies make that a second design.
7. **A composite that carries its own origin validator.** *Recommendation:* the existing offer
   path wins. The endpoint asked to be asked, so it is invoked with `Invocation::stale` as today.
8. **How the trace says it.** *Recommendation:* reuse `TraceState::Revalidated` with the
   existing `revalidated <deadline>` note, plus one new note key naming how many parts were
   rechecked. No new state word, so readers of `state()` need no change.
9. **Does the record count against the byte bound?** *Recommendation:* no. `CacheEntry::bytes`
   stays payload only, as its documentation says. The record is reported beside it in the
   readout, and the cap in decision 5 bounds it.
10. **A cut during the round.** *Recommendation:* discard and invoke the endpoint rather than
    serve the unchanged answer unstored. It costs one run in a rare race, and it keeps "a cut
    invalidates at once" with no exception.
