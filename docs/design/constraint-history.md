# Constraint history: recording the kernel's bottleneck over time

**Status:** design, 2026-11-23 (ledger #4091). The two changes to recording in section 4
(count failures, stamp samples) are built (ledger #4095); section 1 describes the window
before them. The totals of option (b) and the `over=start` face are built (ledger #4096,
`crates/ikigai-core/src/constraint.rs`), and the measurement crate now times them too.
The host sampler (steps 1 and 4 of section 9) is not built here. The measurement
behind section 2 lives beside this note in
[`constraint-history/measure/`](constraint-history/measure/), outside the workspace, and
changes no code in the crates. The code is the spec where the two differ.

---

## The ask

> Use the kernel's bottleneck resource to find performance ideas, by periodically looking at
> what the recent bottlenecks were, perhaps as part of the architecture checks. — Brian,
> 2026-11-23

`urn:kernel:constraint` answers "where is the constraint right now". Nothing answers "where
was it this week". This note says how to record it, what one recording holds, who records,
and how a periodic pass turns a week of recordings into findings.

## 1. What the window is today (verified on main, `0b09719`)

- **Where it lives.** `Kernel::constraint` is a `Mutex<VecDeque<ResolutionSample>>`
  (`crates/ikigai-core/src/kernel.rs:783`), bounded to `CONSTRAINT_WINDOW = 512`
  (`kernel.rs:1862`). Each sample holds the target IRI as a `String`, the elapsed
  milliseconds (`None` on a clockless kernel) and whether the cache served it.
- **What writes it.** `record_resolution` (`kernel.rs:3342`) takes the lock, pops the oldest
  sample when full, and pushes a new one. It is called on the two cache-hit returns
  (`kernel.rs:3101`, `3140`) and once after a computed answer (`kernel.rs:5358`).
- **What reads it.** `constraint_rows` (`kernel.rs:6357`) aggregates the 512 samples by
  target and ranks them by uncached milliseconds, then by uncached count, then by name. Both
  faces list only the top `CONSTRAINT_TOP = 5` (`inspect_graph.rs:39`).

Five properties of the window matter for a history, and the item that asked for this note
names only the first:

1. **It is per process and forgets.** 512 resolutions; a restart empties it.
2. **A read sees five rows.** A resource that is sixth in every window never appears in any
   reading, however long it is sampled.
3. **A failure is not recorded.** The invocation's error arm (`kernel.rs:5101`) returns
   before `record_resolution`, so an endpoint that spends 30 s timing out is invisible to
   the constraint, while the trace records it. Kernel operations are absent too, by design
   (`docs/design/trace-observability.md`, "Kernel operations are trace nodes too").
4. **Time is inclusive.** Sub-requests go through the same `issue_answer` path and are
   recorded too, and a composite's elapsed time includes its children's. A composite and
   its slowest child both rank, and the window cannot say which one spends the time. The
   pass in section 6 has to separate them.
5. **"Uncached" means computed, not uncacheable.** `cache_hit` is `false` for every computed
   answer, including the first computation of a cacheable resource that was then stored. A
   leader can be a resource that is never cached (a volatile dependency), or one that is
   cached and keeps being cut or evicted. The window does not say which. `urn:kernel:uncached`
   does say why, but only for the last 64 resources not stored.

The clock is milliseconds (`Time` is `u64` milliseconds; `SystemClock`, `kernel.rs:88`), so
a 200 µs resolution records 0 ms or 1 ms. Each sample is off by up to 1 ms, but the sum
over many samples is unbiased: a resolution lasting a fraction *f* of a millisecond crosses
a millisecond boundary with probability *f*. Cumulative totals keep this property. A single
sample's milliseconds should not be read as a duration.

## 2. What the window costs a resolution (measured)

The measurement crate builds `ikigai-core` from this checkout and times whole resolutions,
the window's bookkeeping alone (copied from `record_resolution`), and a stand-in for the
cumulative totals of option (b). Run with:

```sh
cargo run --release --manifest-path docs/design/constraint-history/measure/Cargo.toml \
    --bin constraint-history-measure
```

Results from one run on an Intel Core i7-11800H with rustc 1.99.0, release profile, on
2026-11-23. Three runs agreed within 5% on parts 1 and 2:

| What | Per operation |
|---|---|
| `issue`, a volatile endpoint (computed every time) | 906 ns |
| `issue`, a cacheable endpoint (cache hit) | 600 ns |
| `issue`, a template endpoint over 1,024 distinct names | 1,236 ns |
| `issue`, volatile, with a host `Tracer` folding events into totals | 1,137 ns |
| `issue`, cache hit, with the same `Tracer` | 778 ns |
| The window's bookkeeping alone (`record_resolution` copy), one thread | 18 ns |
| The window plus totals keyed by a resident name, one thread | 22 ns |
| Totals with 1,024 names churning through 256 slots (Space-Saving) | 1,159 ns |
| The window's bookkeeping, 16 threads on one lock (wall time per operation) | 240 ns |
| The window plus totals behind a second lock, 16 threads | 294 ns |
| One read of `urn:kernel:constraint`, text face, 512 samples held | 76 µs |

What this says:

- **The window costs about 2% of the cheapest resolution** (18 ns of 906 ns): one lock and
  one `String` copy of the target per resolution. On any endpoint that does real work, it is
  noise.
- **The lock is the real cost, under contention.** Sixteen threads that only record serialize
  at about 240 ns per record, about 4 million records a second for the whole process. A
  kernel resolving that fast on many cores would feel it before it felt anything else this
  note proposes. This note has no measurement of any host's actual rate. Any addition belongs
  under the lock that is already taken, not behind a second one, which added 54 ns here.
- **Totals keyed by a resident name are nearly free** (4 ns over the window, one thread).
- **Totals keyed by the full IRI are not.** A template endpoint makes a new name for every
  argument. Bounding the table by evicting the lightest entry costs a 256-entry scan on each
  new name: 1.16 µs, more than a whole resolution. Full-IRI keys are rejected for this
  reason as well as for privacy (section 4).
- **The host-only route costs 25% of a cheap resolution.** A global `Tracer` that folds
  events into totals works today with no change to core, but every resolution then builds a
  `TraceEvent` (two strings, a notes vector) that the tracer folds and drops. It also takes
  the kernel's single global tracer slot (`Kernel::set_tracer`), which a host may already
  use for ikigai-log.
- **A sampler's read is cheap at any cadence in minutes**: 76 µs every few minutes.

## 3. Recording: three options

### (a) A host-side sampler over the window

Every N minutes, the host reads `urn:kernel:constraint` (Turtle face) and writes one
ikigai-log line per listed target.

- **Hot path:** nothing new. The sampler is an ordinary inspect-capability read.
- **What it loses:** everything between readings, and everything below fifth place. A
  reading covers the last 512 resolutions, so it covers a fraction
  `512 / (rate × N)` of the interval. At 100 resolutions a second and N = 15 minutes, that is
  0.6% of the interval. It sees a resource only if that resource is in the top five of the
  last 512 resolutions when the sampler happens to read. A leader that is slow at 03:00 and
  sampled at 03:15 is gone. A restart between readings is invisible.
- **What it can say:** "X was the constraint when we looked, in k of n readings". This is
  a frequency over sampled moments, not a total.
- **Cost to build:** a host task and a log mapping. No core release.

### (b) Cumulative totals in core

The kernel keeps, since it started, per row: computed count, uncached milliseconds, cache
hits, and a few counters described in section 4. A reading is monotone, so the history of an
interval is the difference of two readings and loses nothing that the kernel saw.

- **Hot path:** measured at 4 ns over today's window when the row is resident and the update
  is under the same lock. No allocation on a resident row.
- **The key decides whether this works.** Keyed by target IRI, cardinality is unbounded:
  every template argument is a new key. That makes the update slow at the bound (measured
  above) and turns the counters into a record of arguments. Keyed by **the endpoint that
  computed the answer**, cardinality is the number of bound endpoints, which is fixed per
  host and small. The resolution has the endpoint in hand when it records
  (`Resolved::endpoint`), and its captured template variables (`Resolved::bindings`). So
  the row can be found by the endpoint's `Arc` address, with a `Weak` to tell a dropped
  endpoint from a new one at the same address. The row's name is computed once, when the
  row is created: the endpoint's `describe().id` and the target's *shape*, which is the IRI
  with each captured variable's value replaced by `{name}` (`urn:file:{path}`, not
  `urn:file:/home/…`). A hit invokes no endpoint, and the hit path does not reliably hold
  the resolved one, so a stored cache entry would carry its row index, and a hit would credit
  the row that computed it. The cost is one
  `u32` per entry.
- **Bound:** at most K rows (256 proposed). Past K, new endpoints fold into one counted
  `(other)` row, with no eviction, so every row stays monotone and differences stay exact.
  Mounted peers and dynamic spaces are the only way to approach K. A mount's row stands for
  everything that went through it, and the peer's own kernel keeps its own totals.
- **Restart:** totals carry the kernel's start time. A reading with a different start time
  than the previous one is a reset, and the interval's difference is the new reading
  itself, as with any monotone counter.
- **What it can say:** "X consumed this many uncached milliseconds over this many
  computations between 03:00 and 03:15". It can also say how much of that time was spent
  computing answers that were then stored, and how much on answers that could not be.

### (c) Both, in order

The sampler of (a) is the component that later reads (b). It is not throwaway work:

1. Build the host sampler against the window now. No core change, data within a week, with
   (a)'s losses stated on every sample (`ik:sampleSource "window"`).
2. Add the totals to core. A new face of the same operation, so nothing that reads the
   window changes (section 4).
3. Point the sampler at the totals. Samples then say `ik:sampleSource "totals"`, and the
   pass weights them over window samples wherever both exist.

**Recommendation: (c).**

### Does (b) belong in core?

The field guide that states this test is not in this repository, so the criteria are stated
here as this note applies them. A change belongs in core when it is about resolution itself
rather than a host's policy, when a host cannot do it from outside with acceptable loss or
cost, and when every host would otherwise build the same thing.

- **About resolution:** per-endpoint compute and cache outcome are facts the kernel observes
  in the act of resolving. Core already keeps the window, and the cache already keeps a
  cumulative counter (`Cache::evictions`). A running total is the same kind of fact.
- **From outside:** a host can do it today only through a global `Tracer`, at 25% of a cheap
  resolution, and only by taking the one tracer slot. A sampler over the window cannot do it
  at all, because the window forgets.
- **Every host:** gonk, cms-web and the layer host would each build the same fold.

So the counters are a kernel concept. **Everything else is host policy**: when to read, the
host's name, where samples go, how long they are kept, and who looks at them. Core gains
counters and one face. It does not gain a timer, a log writer or a host name. Core has no
timer, and this keeps it that way.

## 4. The shape of a sample

### The core face that the sampler reads (proposed)

`urn:kernel:constraint` gains one argument, `over`, with two values: `window`, the default
and today's answer unchanged, or `start`, the totals since the kernel started. Same
capability (`urn:cap:kernel:inspect`), same two faces, uncacheable. Totals list **every**
row, not the top five, because a sampler subtracting two readings needs every row. 256 rows
at most.

Totals per row: endpoint id, target shape, computed count, of which stored, of which failed,
cache hits, uncached milliseconds (computed answers, inclusive of sub-requests), failed
milliseconds, and a count per reason an answer was not stored. The reason kinds are the ones
`urn:kernel:uncached` already names (`src/uncached.rs`): `declared`, `mounted`,
`dependency`, `denied`, `failed`, `one-shot content`, `upstream`, `one-shot body`,
`no-clock`, `expired`, `cut-in-flight` and `policy`. Each is a count, not text. Per kernel:
the start time and the total resolutions recorded.

Two changes to recording come with it, both cheap. Each is a fix to the window too, and
can ship before the totals:

- **Count failures.** Record from the error arm as well, with an outcome of failed. This adds
  one lock on a path that has already failed.
- **Stamp samples.** Keep the end time that `elapsed_since` already reads from the clock, so
  the window can state its span (first and last sample time). This adds 8 bytes per sample
  and no extra clock read.

### One logged sample

One sample is one reading's difference against the previous reading on the same kernel.
Only rows that changed are written: rows whose computed or failed count rose. The fields:

| Field | Property | From |
|---|---|---|
| Host | `ik:sampledHost` | the host's own configured name, never a machine's user or a person |
| Kernel start | `ik:kernelStarted` | the totals (or, for window samples, the sampler's first reading) |
| Interval | `ik:sampleSince`, `ik:sampleUntil` | previous and this reading's time; for window samples, the window's span once stamped |
| Source | `ik:sampleSource` | `"totals"` or `"window"` |
| Resolutions | `ik:sampleResolutions` | difference of the kernel's total |
| Endpoint | `ik:endpointId` | `describe().id` |
| Target shape | `ik:targetShape` | the IRI with captured values replaced by `{name}` |
| Rank | `ik:order` | by uncached milliseconds in this interval, from 1 |
| Computed | `ik:uncachedCallCount` | difference |
| Stored | `ik:storedCallCount` | difference: computed and then cached |
| Hits | `ik:cachedCallCount` | difference |
| Uncached time | `ik:uncachedMillis` | difference, inclusive |
| Mean | `ik:meanUncachedMillis` | uncached time ÷ computed, written for readers that do not divide |
| Failed | `ik:failedCallCount`, `ik:failedMillis` | difference |
| Why not stored | `ik:notStored` → `ik:reasonKind`, `ik:count` | one node per reason kind whose count rose |

`ik:ConstraintTarget`, `ik:order`, `ik:uncachedCallCount`, `ik:cachedCallCount` and
`ik:uncachedMillis` exist in the vocabulary today (`crates/ikigai-vocab/src/vocabulary.ttl`,
from line 825). The rest are proposed terms: `ik:ConstraintSample`, the sample properties,
`ik:endpointId`, `ik:targetShape`, `ik:storedCallCount`, `ik:meanUncachedMillis`,
`ik:failedCallCount`, `ik:failedMillis`, `ik:notStored`, `ik:reasonKind` and `ik:count`.

### IRIs, skolemized

No blank nodes, so that a sample can be cited and re-rendering does not mint new names, as
elsewhere in the vocabulary:

- Sample: `urn:ikigai:constraint:sample:{host}:{started}:{until}`. `{host}` is
  percent-encoded as a name (`encode_name`), and `{started}` and `{until}` are milliseconds.
  The host and the kernel's start time name the counter series. The reading's time names
  the point in it.
- Row: `{sample}:row:{n}`, with `n` the row's `ik:order`.
- Reason: `{sample}:row:{n}:reason:{kind}`, the kind percent-encoded.

```turtle
@prefix ik:  <https://ikigai-rs.dev/ns#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<urn:ikigai:constraint:sample:cms-web:1794729600000:1795161600000> a ik:ConstraintSample ;
    ik:sampledHost "cms-web" ;
    ik:sampleSource "totals" ;
    ik:kernelStarted "2026-11-15T08:00:00Z"^^xsd:dateTime ;
    ik:sampleSince "2026-11-20T07:45:00Z"^^xsd:dateTime ;
    ik:sampleUntil "2026-11-20T08:00:00Z"^^xsd:dateTime ;
    ik:sampleResolutions "91204"^^xsd:nonNegativeInteger ;
    ik:sampleRow <urn:ikigai:constraint:sample:cms-web:1794729600000:1795161600000:row:1> .

<urn:ikigai:constraint:sample:cms-web:1794729600000:1795161600000:row:1> a ik:ConstraintTarget ;
    ik:order "1"^^xsd:integer ;
    ik:endpointId "books-graph" ;
    ik:targetShape "urn:books:graph" ;
    ik:uncachedCallCount "412"^^xsd:nonNegativeInteger ;
    ik:storedCallCount "0"^^xsd:nonNegativeInteger ;
    ik:cachedCallCount "0"^^xsd:nonNegativeInteger ;
    ik:uncachedMillis "398112"^^xsd:nonNegativeInteger ;
    ik:meanUncachedMillis "966.3"^^xsd:decimal ;
    ik:failedCallCount "0"^^xsd:nonNegativeInteger ;
    ik:notStored <urn:ikigai:constraint:sample:cms-web:1794729600000:1795161600000:row:1:reason:dependency> .

<urn:ikigai:constraint:sample:cms-web:1794729600000:1795161600000:row:1:reason:dependency>
    ik:reasonKind "dependency" ;
    ik:count "412"^^xsd:nonNegativeInteger .
```

The numbers are illustrative, shaped after the cms-web books-graph regression that
`src/uncached.rs` records (about 20 µs to about 1 s per read, 2026-08-13). They are not a
measurement.

How a sample becomes an ikigai-log line is ikigai-log's mapping to own. Its vocabulary
table maps terms straight through, as it does for trace notes
(`docs/design/resolution-scope.md`), so each proposed term above needs one entry there. The
log line's actor is the host's sampler principal. The record is "this host observed this
interval", not something a person did.

### A week's recurring leaders, in SPARQL

Over the union of a week's log segments, loaded as one dataset: for each host, endpoint and
shape that led at least three intervals, how many intervals it led, and its uncached time
and computations over the whole week, including intervals it did not lead.

```sparql
PREFIX ik:  <https://ikigai-rs.dev/ns#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

SELECT ?host ?endpoint ?shape ?intervalsLed
       (SUM(?ms) AS ?weekUncachedMillis)
       (SUM(?calls) AS ?weekComputed)
       (SUM(?ms) / SUM(?calls) AS ?weekMeanMillis)
WHERE {
  {
    SELECT ?host ?endpoint ?shape (COUNT(DISTINCT ?led) AS ?intervalsLed)
    WHERE {
      ?led a ik:ConstraintSample ;
           ik:sampledHost ?host ;
           ik:sampleSource "totals" ;
           ik:sampleUntil ?ledUntil ;
           ik:sampleRow ?leader .
      ?leader ik:order 1 ;
              ik:endpointId ?endpoint ;
              ik:targetShape ?shape .
      FILTER (?ledUntil >= "2026-11-16T00:00:00Z"^^xsd:dateTime
           && ?ledUntil <  "2026-11-23T00:00:00Z"^^xsd:dateTime)
    }
    GROUP BY ?host ?endpoint ?shape
    HAVING (COUNT(DISTINCT ?led) >= 3)
  }
  ?sample a ik:ConstraintSample ;
          ik:sampledHost ?host ;
          ik:sampleSource "totals" ;
          ik:sampleUntil ?until ;
          ik:sampleRow ?row .
  ?row ik:endpointId ?endpoint ;
       ik:targetShape ?shape ;
       ik:uncachedMillis ?ms ;
       ik:uncachedCallCount ?calls .
  FILTER (?until >= "2026-11-16T00:00:00Z"^^xsd:dateTime
       && ?until <  "2026-11-23T00:00:00Z"^^xsd:dateTime)
}
GROUP BY ?host ?endpoint ?shape ?intervalsLed
ORDER BY DESC(?intervalsLed) DESC(?weekUncachedMillis)
```

For window samples, swap `"totals"` for `"window"` and read `?intervalsLed` as readings led.
Their sums are sums over the moments sampled, not over the week.

Both blocks above are checked by `--bin check-query` in the measurement crate (oxigraph
0.5.13). It loads the example and two copies shifted by 15 and 30 minutes, then runs the
query. On 2026-11-23 it returned one row: `cms-web`, `books-graph`, 3 intervals led,
1,194,336 ms over 1,236 computations, mean 966.3 ms. CI does not run it.

## 5. Privacy

Target IRIs can carry arguments: a path, a search term, an account name in a template
variable. Named request arguments (`Request::args`) are never in the window or the totals.
They are recorded nowhere here.

What is recorded:

- **Endpoint id**, verbatim. It is written by the module's author (`books-graph`,
  `kernel-constraint`) and names code, not people.
- **Target shape**: the IRI with every captured template value replaced by its variable's
  name. `urn:file:{path}` is recorded, and `urn:file:/home/alice/notes.md` is not. A name
  bound exactly (`Exact`) has no captured values and is recorded as it is bound, because a
  host author wrote it into the binding table.
- **Counts and milliseconds**, aggregated per row over an interval.
- **The host's configured name** and the kernel's start time.

What is not recorded: argument values, template values, capabilities or principals, request
ids, bodies, and the dependency names inside an uncached reason. The reason is recorded as a
kind and a count. The live `urn:kernel:uncached` still names the dependency for whoever holds
inspect on that host.

**Not hashed, dropped.** A hash of a template value is not anonymous: the values are usually
guessable (account names, paths), so a dictionary reverses the hash. Hashing would keep the
risk and add the appearance of safety. The shape keeps only what the binding table already
says.

**What identifies a person: nothing, given one condition.** Rows are aggregates per endpoint
over minutes, with no principal and no value. The one residual risk is an `Exact` binding
whose name itself names a person (`urn:user:alice:inbox` bound exactly). That is a host's
binding decision, visible in its binding table, and the sampler can refuse it: drop any row
whose endpoint is listed in a host-side deny list, and record it as dropped. The window
itself, today, holds full target IRIs in memory and shows them to inspect holders. Nothing
proposed here widens that, and the shapes narrow what leaves the process.

## 6. Which hosts sample, and how often

Sample where a kernel runs long enough for a week to mean something, and serves traffic
whose cost someone would act on:

| Host | Sample | Why |
|---|---|---|
| gonk | yes | a long-running host; start here, because it is the first sampler built (section 9) |
| cms-web | yes | a long-running host, and the one where an uncached composite already cost a 50,000× slowdown (2026-08-13, `src/uncached.rs`) |
| the layer host | yes | a long-running host serving layers; where another host mounts it, its cost shows there as one mount's row, so sample it to see inside |
| the cli's `serve` | opt-in only, off by default | it runs while a developer runs it: short-lived, with unrepresentative load, on a person's machine |

**Cadence: every 15 minutes**, from the host's own scheduler, and once more at shutdown when
the host can. With totals, the cadence sets only the time resolution of the history, not
what it loses. 15 minutes gives 672 intervals a week per host, and each interval writes
only the rows that changed, typically tens. With window samples, the cadence is a coverage
choice. Read faster on a quiet host (the window spans more of the interval) and do not
bother reading faster on a busy one (the coverage formula in section 3 stays small). Whoever
holds the sampler's capability for a host is the host's operator, not this note.

## 7. The periodic pass

A pass runs weekly, or as one of the architecture checks when those run.

**Inputs:** one week of samples from every sampling host, the query of section 4, and access
to each host's live `urn:kernel:uncached`, `urn:kernel:cached` and traces for follow-up.

**For each recurring leader** (led at least three intervals, or is in the top three of a
host's week by uncached milliseconds):

1. **Inclusive or its own?** Find the row's children: rows on the same host whose endpoint
   the leader sources (from a trace of one read). If a child's uncached time explains most
   of the leader's, the child is the constraint and the leader is only where it shows.
   Inclusive time makes every ancestor of a slow resource rank (section 1, item 4).
2. **Who reads it:** the endpoints and doors that issue it (trace of one read, or the host's
   access log), and how many reads in the week (`ik:uncachedCallCount` plus
   `ik:cachedCallCount`).
3. **Why it is computed:** stored computations (`ik:storedCallCount`) point at cuts,
   eviction or short expiry. Check `urn:kernel:cached` and what cuts its thread. Unstored
   ones point at the reason kinds, and `urn:kernel:uncached` names the dependency live.
4. **Cost per read:** `ik:meanUncachedMillis` over the week, and the spread across intervals
   (a mean hides a bimodal endpoint).
5. **What a fix would save:** computations × mean, for each candidate. A cache that would
   hold (the reason kind gone) saves nearly all of it. A cheaper composition (a narrower
   source, a pinned corridor for a volatile dependency, splitting the volatile part out of
   a composite) saves the share the trace attributes to the part replaced. State the saving
   in milliseconds per week and per read.

**The skeptic rule: no measurement, no finding.** A finding cites the samples (their IRIs)
and a reproduction. The reproduction is a trace or a timed run that shows the cost, and it
is repeated after the proposed fix to show the saving. A leader that cannot be reproduced
is noted as not reproduced and is not filed. A saving that was estimated and not measured
is stated as an estimate. A leader that is the system's intended work (an endpoint whose
job is the expensive computation, read as rarely as it needs to be) is not a finding. The
constraint is somewhere; the pass asks whether it is somewhere it should not be.

**Where findings go:** one ledger item per finding, in the repository that owns the
endpoint, labeled `perf`, with the sample IRIs, the reproduction and the measured or
estimated saving. Projects outside ikigai-rs are not filed against. A finding in a
dependency outside the organization is written up in the owning ikigai-rs repository.

## 8. Not decided here

- **K** is 256 by guess. The first week of window samples gives each host's count of
  distinct endpoints. Set K above the largest with room.
- **Whether `over=start` or a separate operation.** An argument keeps one description and
  one capability, and adds one value to document. A separate `urn:kernel:throughput` would
  keep `constraint` exactly as it is. This note prefers the argument, and the first
  implementation item decides. *Decided (ledger #4096): the argument.* That item also
  found that an endpoint's `Arc` address alone does not identify it: a dynamic space and
  a throttle governor wrap the endpoint in a fresh `Arc` on every resolution, which would
  make one row per request and fill K within 256 requests. So an address the kernel has
  not seen is matched to a row by its name (id and shape) before a row is made, and the
  rows are bounded by distinct names, not allocations.
- **Retention** of samples is ikigai-log's policy. A pass needs one week. A trend across
  releases needs months, and a sample is small.
- **Self time.** Recording a composite's time minus its children's would remove step 1 of the
  pass, but needs per-resolution accounting of children's elapsed time on the hot path. Not
  proposed until the pass shows that inclusive time misleads in practice.

## 9. Build order (for the items that follow, not filed by this note)

1. Host sampler over the window (gonk first), with its ikigai-log mapping. No core change.
2. Core: record failures, and stamp window samples (section 4). Small, fixes the window
   today.
3. Core: totals keyed by endpoint and shape, under the existing lock, and the `over=start`
   face. Measure again with the crate beside this note. The bar is no more than 10 ns over
   today's window on one thread, and no regression under 16 threads.
4. Sampler reads the totals. The first weekly pass runs over a week of totals samples.
