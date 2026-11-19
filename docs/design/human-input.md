# Human input: a pipeline stage that waits for a person

**Status:** design only (ledger #3826). Nothing here is built, and nothing here adds a crate,
an endpoint or a type. The decisions it needs are listed at the end, for Brian.

**Anchor types (existing):** `ikigai_core::{CutListener, ListenSpec, CAP_LISTEN, Kernel,
Invocation, IssueContext, Error, FailureClass, Capability}`.
**Companions:** `docs/design/principal-capability.md` (who a request runs for),
`docs/design/cache-ejection.md` §2 (a capability fingerprint is not an identity),
`docs/design/sub-request-authority.md` (attenuation), `docs/design/provenance-labels.md`.

---

## The idea

ikigai runs many `a | b | c` pipelines. Sometimes stage `b` needs a person: an approval, a
configuration choice, or picking fields from a JSON stream so that `b` becomes a jq query for
`c`. A window collects the input and the pipeline goes on.

The design question is what that looks like from the resource perspective. The claim this
document tests is: **the person's answer is a resource, not a side effect of a blocking call.**
If it holds, a rerun reads the answer from the cache instead of asking again, editing the answer
cuts a golden thread so the stages after it recompute, and the answer is a signed, addressable
record of who decided what, having seen which question.

It holds, with two corrections to the starting model (the names, and how a waiting face hears
the answer arrive) and one rule the starting model did not state (what goes into the question's
digest). All three are below.

## What was checked in the code

These claims from the item were checked against `main` at 975aaf4 before writing:

- `CutListener::wait` (`crates/ikigai-core/src/listen.rs`) is a runtime-free future that parks
  on the `Waker` it is polled with. One waiter at a time: a second concurrent `wait` replaces
  the first one's waker.
- Only a host can register one: `Kernel::listen(spec, &capability)` refuses with `Denied`
  unless the capability grants `CAP_LISTEN` (`urn:cap:kernel:listen`). `Invocation` has no
  listen method, so an endpoint cannot park on a cut today.
- **A non-root listener hears a cut of thread `T` only when some cache entry computed under its
  own capability fingerprint depends on `T`.** This matters for option B below: an error that is
  not cached leaves nothing resting on `T`, so a listener that only saw the error would never
  hear the answer arrive.
- Errors are never cached unless their endpoint states a failure expiry
  (`Invocation::failure_expires_at`), and a composite that swallows a failed sub-request and
  returns a cacheable fallback is forced uncacheable.
- A run carries a deadline (`not_after`) and a retry pool in its `IssueContext`, inherited by
  every sub-request; depth is bounded by `Kernel::with_max_depth` (default 64).
- Identity travels as a capability scope, `urn:cap:principal:<iri>`, minted by the door that
  authenticated the party.

---

## 1. The resource map

### Naming, against the house rule

The house rule is the mechanical route `/noun/partition/key -> urn:noun:partition:key`: the
type first, singular, and every partition by a named key (`urn:account:id:{sub}`).

The starting model's `urn:iki:ask:question:id:{digest}` puts a product and a module (`iki:ask`)
ahead of the type, so its route would be `/iki/ask/question/id/…`. The model's answer name has a
second problem: `urn:iki:ask:answer:id:{digest}` does not say which digest. If it is the
answer's own content digest, the answer cannot be edited (an edit is a new name, and nothing
cuts); if it is the question's digest, the key should say so. Proposed:

| Resource | Name | Kind |
|---|---|---|
| A question | `urn:question:id:{digest}` | atom, immutable (content-addressed by what is asked) |
| Its answer | `urn:answer:question:{digest}` | atom, mutable: the only state |
| A signed receipt of one answer | `urn:receipt:id:{content-digest}` | atom, immutable (ikigai-sign's output) |
| The open questions a host may show | `urn:question:status:open` | set (a view over questions with no answer) |
| Stage `b` | a composition over the answer, e.g. `urn:json:query?src=…&q=…` | composition, cached |
| A parked run (option C only) | `urn:run:id:{run}` | atom, mutable |

`urn:answer:question:{digest}` reads as "the answer, partitioned by the question it answers",
which is the rule exactly, and it keeps one answer per question, which is what makes "editing
the answer cuts its thread" true. Whether `question` and `answer` are too generic as top-level
nouns, against an `ask` prefix, is decision 1.

### The question: what it holds, and what its digest covers

A question is an atom holding:

- **the asker**: the principal the run is for (`urn:cap:principal:…` from its capability) and
  the pipeline or plan step that issued it;
- **the purpose**: what the answer will be used for, as a name (for the jq case, "`q` of
  `urn:json:query` feeding `c`");
- **the answer's shape**: a SHACL shape the answer must conform to;
- **context for the person**: a sample of the input, the plan step's label, any prose.

**The digest is of the asker, the purpose and the shape, and never of the sample.** A sample is
there so the person can see what they are deciding about, and if it were in the digest every
new batch of data would ask again. The jq case goes one step further, as the item proposes: the
shape is derived from the input's **inferred schema**, so the question's digest is a function of
the schema, and a person answers once per data shape.

The asker and purpose must be in the digest, and this is the rule the starting model did not
state. If they were only context, two different pipelines asking about the same schema would
share an answer, and an approval given to one would silently authorize the other. Whether two
runs of the **same** pipeline for different principals share an answer is decision 2.

### The answer: the only state

The answer is an atom written by a `Sink` under a capability that only a person's door can
mint (§3, Authority). The kernel's auto-cut on that `Sink` cuts the answer's thread, which is
how everything below learns it changed. The answer holds:

- the value, conforming to the question's shape (for the jq case, a jq program as text);
- the question digest it answers, so the person's decision names what they saw;
- optionally a signature (ikigai-sign) over the value, the question digest, the principal and
  the time, stored as `urn:receipt:id:{content-digest}`: a PROV-O record that this person
  approved this value for this question at this time. The capability fingerprint cannot serve
  that purpose; it is a cache partition, not an identity (`cache-ejection.md` §2).

Reading an answer that does not exist yet is a **cacheable** `Exists → false` (or a cacheable
"absent" representation) that rests on the answer's thread. That one detail is what lets option
B work; see §2.

### Stage `b`: a composition

`b` is a composition over the answer. With an answer present it is an ordinary cached read
whose representation rests on the answer's thread (golden-threaded), so:

- a rerun with the same question is a cache hit, and nobody is asked again;
- an edit to the answer cuts its thread, `b`'s entry goes stale, and `c` recomputes on its next
  read.

For the jq field-picker, the answer is a jq **program**, and `b` is
`urn:json:query?src=<a's output>&q=<the answer's IRI>`: code as a resource. `q` names the
answer instead of inlining its text, so the query's cache entry rests on the answer's thread.

With no answer present, `b` creates the question (idempotently, by digest) and fails with a
typed, uncached Pending error that names the question and the answer IRI. How the run waits from
there is §2.

### Corridors and views

- **Corridor:** the answer `Sink` is reachable only through a door that mints the person's
  capability (gonk's passkey door, a desktop host's local session). It is never reachable through
  the corridor an agent or pipeline runs in (§3, Authority).
- **Views:** a question carries a shape, so its presentation is a view the host chooses (§4),
  never something `b` decides.

---

## 2. How a request is parked while it waits

### A. Park inside the request

The endpoint for `b` awaits until the answer's thread is cut, then reads it and returns.

**What it needs that does not exist.** An endpoint-reachable seam, for example
`Invocation::await_cut(&answer_iri)`, which returns when that name is cut. Its authority rule
would have to be the same as a listener's, scoped smaller: a capability granting a new
`urn:cap:kernel:await` (not `CAP_LISTEN`, which hears prefixes), and the request hears only the
exact name it awaits, which it already holds. That rule is safe, since it discloses nothing it
did not already name, but it is new surface.

**What the parked request holds.**

- one depth slot and one stack of futures for every stage from the host down to `b`;
- the run's deadline: `IssueContext::not_after` keeps running, so a person who takes ten
  minutes turns a short deadline into a `Timeout`, and that `Timeout` is **transient** and
  retried by an overlay, which re-asks: the worst possible reading of "the person has not
  answered";
- the client's connection: an HTTP request open for minutes (proxies cut at 60 to 120 s), or an
  MCP tool call whose client has its own timeout;
- the one waker `CutListener::wait` supports per listener, so two parked requests on one
  listener would starve each other unless each gets its own.

**Cancellation.** In Rust, by dropping the future: the client disconnects, the host drops the
request, the `await_cut` future is dropped. That is clean inside one process and invisible to a
remote client unless the face maps a disconnect to a drop.

**Verdict.** It fits only an interactive, local, short wait (the desktop host, the cli), and
even there it has the deadline problem. It needs a new seam in core. Not recommended as the
general mechanism.

### B. Park at the edge (recommended)

The request ends. `b` returns a typed, uncached **Pending** error naming the question and the
answer IRI. The host or face (the cli, gonk, the plan executor) holds a `CutListener` on the
answer's thread and re-issues the original request when it is cut. Every stage before `b` is a
cache hit, so the re-issue replays from the cache: **the cache is the continuation**, and nothing
is captured but names.

**The authority catch, and its fix.** A non-root listener hears a cut of `T` only if an entry
computed under its own capability rests on `T`. Pending is an error and is not cached, so after a
Pending nothing of the run's rests on the answer's thread, and the listener would never hear the
answer arrive. The fix uses existing parts only: before parking, the face reads the answer once
under the run's capability (`Exists`, or a `Source` that returns a cacheable "absent"). That entry
rests on the answer's thread, so the listener now hears its cut, and the face has learned no name
it did not already hold, which is the listener's own disclosure rule. No change to core.

**What it holds.** A listener and a name. No depth slot, no stack, no open connection: a cli
can print "waiting for an answer to urn:question:id:…" and park; a gonk page can show the
question and be pushed the change; the plan executor can hold many.

**Costs.** The re-issue recomputes anything before `b` that was not cacheable (a streamed source
with no cache entry is re-read, which for the jq case is the point: the stream is live). A host
must exist to hold the listener, so a face with no host process cannot use it alone. And
"re-issue when cut" must be bounded: a cut with a still-absent answer (a retraction) re-parks
instead of looping.

**Verdict.** It fits the interactive window now, and a remote approver within the life of the
host process, with no change to core beyond the Pending kind itself.

### C. Park as data

The run itself is an atom, `urn:run:id:{run}`, with state `pending`, the request it will
re-issue, and the answer IRI it waits on. Whatever process holds a listener (or polls) resumes it
when the answer arrives. It survives restarts and moves between machines. ikigai-script's runs
atoms (`urn:script:{name}:runs`) are the nearest existing shape.

**Costs.** A run atom is state that outlives the person who started the run, so it needs an owner,
an expiry and a way to be abandoned; and its stored request must be re-issued under a capability
that still holds when it resumes, which means storing a reference to an authority, not the
authority itself (decision 4).

**Verdict.** It fits approvals that take hours or days, and unattended workers. It is B with the
parked state written down: the Pending error is the same, and the run atom is what a host writes
instead of holding a listener in memory.

### Which fits which case

| Case | Mechanism | Why |
|---|---|---|
| An interactive window, now (desktop, cli) | **B** | the host is already running and holds the listener; no connection stays open |
| A remote approver, later (gonk, a layer) | **B**, falling back to **C** past the host's life | the face shows the question; a long wait is written down |
| An unattended worker | **C**, or refuse (§3, No display) | nobody is present; the run must survive the worker's exit |
| A short, local, synchronous embedding | A, only if decision 3 builds the seam | the one case where holding the stack is cheaper than replaying |

---

## 3. The hazards, each with a rule

### No display

CI, a worker machine, a server, a launchd service: the host has no person to show the question
to. **Rule: a host that cannot present a question either routes it to a view it does have (a
layer, gonk) or fails the run with Pending at once, naming the question. It never waits on a
display it does not have.** A host declares which views it can present; a run started on a host
with none fails before its first stage if its plan says it will ask (see Plans).

### Errors

Two kinds, neither ever cached:

- **Pending** (new): "an answer to question `Q` is needed, at `A`". Class: not transient (a
  retry overlay must not re-issue it; nothing changes until a person acts), and not
  `RetryAfter` (there is no stated time). It needs a class of its own or a reading of
  `Permanent` that tells callers "a change of state clears this", like `Conflict`. Its message
  starts with words: `pending: an answer is needed for urn:question:id:… (approve the deploy of
  …)`.
- **Declined** (new): "the person said no, to question `Q`". Class `Permanent`; never cached,
  because the person may change their mind and edit the answer, and a cached refusal would
  outlive the edit. Message: `declined: … declined urn:question:id:…`.

No existing kind fits either. `Timeout` and `Unavailable` are transient, which would make an
overlay re-ask. `Conflict` is close to Pending ("the current state refuses it") but says the
request is wrong, not that it is incomplete. `Denied` is the capability floor's word, and
`Refused` a remote authority's. Whether Pending and Declined are new `Error` variants or one new
variant with a reason is decision 5.

**Closing the window is Declined, never a timeout.** A window closed without an answer writes
nothing and the run reports Declined to its host; a host-side wait that runs out is Pending, not
success and not a transient timeout. No path turns "nobody answered" into an answer.

### Phishing

A window that a pipeline can open is a phishing surface. **Rules:**

- Every view shows, before any input: who is asking (the principal), through which pipeline or
  plan step, and what the answer will be used for (the question's purpose). These come from the
  question atom, which is content-addressed, so a view cannot be shown one question and record
  an answer to another.
- Opening a view is a capability of the host (a proposed `urn:cap:ask:present`), not something
  `b` can trigger: `b` only creates a question and fails Pending. Which questions reach a person
  is the host's routing decision.
- A view labels itself as coming from the host, in the host's own frame, never in content the
  question supplies.

### Authority

**Rule: the answer `Sink` requires a capability (a proposed `urn:cap:ask:answer`) that only a
door authenticating a person mints, and the capability a pipeline or agent runs under never
holds it.** Attenuation is strictly narrowing (`sub-request-authority.md`), so nothing downstream
can add it. An agent driving the pipeline can read the question and wait, but cannot write the
answer, so the human gate is enforced, not advisory. The receipt's signature names the person,
since the capability alone does not.

### Plans

**Rule: a plan says before a run that it will pause for a person, and for which shape.** A plan
step's `urn:plan:requires` (or an equivalent `urn:plan:asks`) names the question's shape and
purpose. That lets the plan executor check, before the first stage, that the host can present it
(No display), that some principal can answer it (Authority), and whether an answer already
exists, in which case the run will not pause at all. Which predicate carries it is decision 6.

### Accessibility

The GPUI version pinned today has no AccessKit, so a blocking GPUI window is a hard barrier for
screen-reader users. **Rule: the htmx view in gonk is always available for every question, and a
host that offers the GPUI window offers the htmx view beside it, so nobody is offered only the
GPUI window.** The cli prompt is plain text and works with a screen reader. Every view states the
question's purpose and asker in text, not by color or position alone, following
`ACCESSIBILITY.md`.

### Concurrency

- **Several questions at once:** each is its own atom and its own thread; a host lists them from
  `urn:question:status:open` and a person answers in any order. Each parked run re-issues when its
  own answer's thread is cut.
- **The same question from two runs:** they compute the same digest, so they create the same
  question atom (the create is idempotent) and wait on the same answer. One answer releases both.
- **An answer after the run gave up:** it is not lost. It is written, cut and stored; the next run
  that asks the same question finds it and does not ask. A face that already reported Declined or
  Pending for the old run does not resurrect it (under C, the run atom says `abandoned`).

---

## 4. Where the question shows

The question's SHACL shape is the input to every view; ikigai-ui's form-from-shape work is the
likely base for the three graphical ones. All four show the asker, the pipeline and the purpose
first (§3, Phishing).

**A GPUI window on the desktop.** The desktop host renders the shape as a native form. For the
jq case: a tree of the sample's fields with a checkbox per leaf, and a preview of the projected
sample. *Needs, that the others don't:* a running desktop session and a host that owns a window;
AccessKit before it can be the only view, which it may not be today (§3, Accessibility).

**An htmx form in gonk.** gonk serves the shape as an HTML form behind its passkey door, which is
also where the person's capability is minted. Submitting is the answer `Sink`; the page is pushed
the cut. *Needs:* an authenticated session (the door), and CSRF protection on the `Sink`. It is the
always-available view.

**A prompt in the cli.** The cli renders the shape as questions in the terminal: numbered fields
to tick, or a jq program typed or edited in `$EDITOR`. *Needs:* a terminal that is interactive
(a TTY); with none it routes elsewhere or fails Pending (§3, No display). The cli must say which
capability it writes under, since it is both the person's door and the pipeline's host.

**A post to a layer, for remote approval.** The host `out`s the question to a tuplespace layer;
one approver `take`s it, which gives "one approver takes it, with a receipt" for free, and
answers through their own door. *Needs:* a layer reachable by both sides, a way to return a taken
question that nobody answered, and the signed receipt, because the approver is on another machine
and the receipt is the only proof that reaches back.

---

## 5. Prior art, briefly

- **MCP elicitation.** A server asks the client's user for structured input, by JSON Schema, in
  the middle of a tool call, and the user accepts, declines or cancels. Our MCP projection can map
  a question onto it directly: the SHACL shape is projected to JSON Schema (for the shapes that
  have a JSON Schema equivalent), accept is the answer `Sink`, and decline and cancel are
  Declined. The difference: in MCP the answer is a return value of one call and gone afterward;
  here it is written as a resource, so the next run does not ask. Elicitation is park-inside-the-
  request (option A), with the MCP client's timeout as the bound.
- **Workflow engines.** A BPMN user task is a task record that waits for a person to complete it;
  a Temporal signal delivers external input to a durable workflow that replays its history to
  resume. Both are option C. Temporal's replay is the closest relative of "the cache is the
  continuation", with one difference: Temporal replays an event history, and we replay cached
  reads, which other runs share.
- **Where ikigai differs:** the answer is a cached, addressable, optionally signed resource that
  later runs reuse, keyed by the shape of what was asked, so asking once per data shape falls out
  of the cache rather than being built.

---

## 6. Worked example: the jq field-picker

The pipeline is `a | b | c`: `a` reads an API's JSON stream, `b` projects fields, `c` consumes
the projection.

1. **The stream arrives at `b`.** `b` infers the stream's schema from its first records and
   computes the question's digest from the asker, the purpose ("`q` of `urn:json:query` feeding
   `c`") and the schema-derived shape. It creates `urn:question:id:{d}` if absent, with a sample
   of the records as context. It reads `urn:answer:question:{d}`: absent. It fails **Pending**,
   naming both.
2. **The question shows a field tree.** The host, having read the answer's absence under the
   run's capability (so its listener will hear the cut), presents the question in a view it can
   show: the GPUI window and the htmx form on the desktop, the htmx form alone in gonk, a prompt
   in the cli. Each shows the schema's field tree, built from the sample, with who is asking and
   what for.
3. **The person ticks fields.** The view turns the ticks into a jq program, for example
   `{id, name, price: .offer.price}`, and shows it, since the program is what will be stored.
4. **The answer is a jq program.** The view's door writes it with a `Sink` to
   `urn:answer:question:{d}` under the person's capability, signed if requested. The auto-cut
   cuts the answer's thread.
5. **`c` receives the projected stream.** The host's listener hears the cut and re-issues the
   pipeline. `a` is replayed or a cache hit; `b` now reads the answer and resolves
   `urn:json:query?src=…&q=urn:answer:question:{d}`, cached and resting on the answer's thread;
   `c` receives the projected stream.
6. **A second stream with the same schema flows through without asking.** It infers the same
   schema, computes the same digest, finds the answer, and `b` is an ordinary read. If the person
   later edits the program, the answer's thread is cut, `b`'s entries go stale, and `c`
   recomputes on its next read. A stream with a new field has a new schema, a new digest and a
   new question (whether a superset schema may reuse the old answer is decision 7).

---

## 7. Decisions for Brian

1. **Names.** `urn:question:id:{digest}` and `urn:answer:question:{digest}` (type first, per the
   route rule), or keep a module prefix as `urn:ask:question:id:…`, accepting that `ask` is then
   the type? The starting model's `urn:iki:ask:…` fits neither.
2. **What the digest covers.** Proposed: asker, purpose and shape, never the sample. Should the
   asker be the pipeline (so every principal running it shares one answer) or the principal (so
   each person answers for themselves)?
3. **Parking.** Adopt B as the mechanism, with C for long waits and unattended runs, and do not
   build the endpoint-level `await_cut` seam of option A? Or build A for the local, synchronous
   case too?
4. **Run atoms (C).** Where do they live (ikigai-script's runs atoms, or a general `urn:run:id:…`),
   and under what authority does a resumed run re-issue, given that a capability cannot be stored
   and replayed as is?
5. **Errors.** Pending and Declined as two new `Error` variants, or one variant with a reason?
   And does Pending get a `FailureClass` of its own ("waits on a person"), or reuse `Permanent`
   with the rule "a change of state clears it", as `Conflict` does?
6. **Plans.** Which predicate says a plan will ask: `urn:plan:requires` with a question shape, or a
   new `urn:plan:asks`?
7. **Schema evolution.** When a stream gains a field, does its new schema ask again (the safe
   default, proposed) or may an answer for a subset schema be reused?
8. **Receipts.** Is signing the answer required for remote approvals and optional locally, or
   always optional?
9. **Capabilities.** Adopt `urn:cap:ask:answer` (write an answer, minted only by a person's door)
   and `urn:cap:ask:present` (a host may show questions), or fold them into existing scopes?
10. **The GPUI window.** Ship it only beside the htmx view until the GPUI pin has AccessKit, as
    §3 proposes, or hold it back until then?
