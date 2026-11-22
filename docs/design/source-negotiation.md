# Source negotiation on `as=`

**Status:** design, 2026-11-21. Nothing in the kernel changes with this note. One
proof lives in a test, `crates/ikigai-core/tests/source_negotiation.rs`, built on the public
API only. Covers ledger #2153, #647 and #1465, which all ask the same question.

## The question

When a `Source` request carries `as=<type>`, the endpoint answers in `<type>`, and a
transreptor chain from that type to `<type>` is mounted, should the kernel run the chain?
Today it does not. The kernel transrepts only `Meta` answers. So every endpoint that wants
"any text/csv resource, as application/json" plans and drives the chain itself, and whether
that works depends on each consumer, not on the kernel.

## What the kernel does today

- **`Meta`** is negotiated. `meta_answering` (`crates/ikigai-core/src/kernel.rs:4257`) asks
  the renderer for the type. If the renderer cannot emit it, `transrept_meta`
  (`kernel.rs:2507`) plans `text/turtle → as` in the *resolved* scope, under the request's
  `lossy=` policy (`meta_policy`, `kernel.rs:7175`), and runs each step as a sub-request
  one level deeper. The steps' threads and the binding thread (`BINDINGS_THREAD`) hang the
  result. With no plan it hands back the canonical Turtle and says so in `repr_type`. With
  only a lossy plan it refuses, naming the lossy step and the consent (`lossy=allow`).
- **`Source`** is not negotiated. `as` is an ordinary argument. It is part of the request's
  identity and so of its cache key. The endpoint may read it or ignore it. An endpoint
  that ignores it answers its own type, and the kernel passes that answer through
  unchanged. The first test in the proof, `today_a_source_answer_ignores_as_even_with_a_plan_mounted`,
  pins this: a csv → json plan is selectable, and `as=application/json` still answers
  `text/csv`.
- **Kernel operations** serve their faces themselves and refuse an `as` they do not serve
  (ledger #2143: `kernel_single_face`, `kernel.rs:6979`, and `kernel_face`, `kernel.rs:6997`;
  pinned by `tests/kernel_faces.rs`).
- **Two plan executors.** `transrept_meta` drives its steps as buffers.
  `Invocation::transrept` (`crates/ikigai-core/src/endpoint.rs:1913`) is the public one. It
  streams a body to an incremental step and collects it for any other step. Planning goes
  through `Invocation::select_transreptor` (`endpoint.rs:2062`), which plans in the
  invocation's chain.

## Inventory: today's copies of the loop

Found in the public repositories of the `ikigai-rs` organization at the commit named on
each line. Tests and doc examples are left out. "Hand-rolled" means the endpoint copies the
step loop: parse the step IRI, issue `Source` with `content` and `as`, and repeat.
`inv.transrept` means it uses the kernel's executor.

| Repository and location | What it converts | Loop | Kind |
|---|---|---|---|
| [ikigai-engine `crates/ikigai-engine/src/plan_space.rs:712`](https://github.com/ikigai-rs/ikigai-engine/blob/08f9dd2a809653a3b4c5a96270f7df9e37995aed/crates/ikigai-engine/src/plan_space.rs#L712) | a plan's result → the caller's `as` | hand-rolled | serves `as` |
| [ikigai-name `src/document.rs:275`](https://github.com/ikigai-rs/ikigai-name/blob/99fdf2016faf6575305a3188f318d95a03ab197a/src/document.rs#L275) | a stored document → the caller's `as` | hand-rolled | serves `as` |
| [ikigai-linkeddata `crates/ikigai-sniff/src/lib.rs:430`](https://github.com/ikigai-rs/ikigai-linkeddata/blob/b87e933f6c748af1148a0982c6b371302061595d/crates/ikigai-sniff/src/lib.rs#L430) | sniffed bytes → the caller's `as` (it is a transreptor) | `inv.transrept` | serves `as` |
| [ikigai-json `src/lib.rs:129`](https://github.com/ikigai-rs/ikigai-json/blob/4f8c4c9e8a162b92a5351425b7d72f1e0e17b438/src/lib.rs#L129) | a `src` → `application/json` | hand-rolled | reads a source |
| [ikigai-json `src/streaming.rs:59`](https://github.com/ikigai-rs/ikigai-json/blob/4f8c4c9e8a162b92a5351425b7d72f1e0e17b438/src/streaming.rs#L59) | a `src` → `application/json`, streamed | `inv.transrept` | reads a source |
| [ikigai-linkeddata `crates/ikigai-sparql/src/lib.rs:1053`](https://github.com/ikigai-rs/ikigai-linkeddata/blob/b87e933f6c748af1148a0982c6b371302061595d/crates/ikigai-sparql/src/lib.rs#L1053) | a source → the first RDF type a plan reaches | hand-rolled | reads a source |
| [ikigai-ui `src/view.rs:353`](https://github.com/ikigai-rs/ikigai-ui/blob/f9decdec367e2552a2101b0f2d6f81f4a60e74e3/src/view.rs#L353) | a resource → JSON-LD | `inv.transrept` | reads a source |
| [ikigai-diagram `src/lib.rs:422`](https://github.com/ikigai-rs/ikigai-diagram/blob/c7906b85557eec701b50d813a7038efe906c4a62/src/lib.rs#L422) | a resource → Turtle, falling back to the bytes as served | hand-rolled | reads a source |
| [ikigai-toolbox `src/lib.rs:347`](https://github.com/ikigai-rs/ikigai-toolbox/blob/64968f1198c68f1633de71055b541d443c21908a/src/lib.rs#L347) | a resource → Turtle | hand-rolled | reads a source |
| [ikigai-tutorial `crates/tic-tac-toe/src/declared.rs:307`](https://github.com/ikigai-rs/ikigai-tutorial/blob/37a36c18787ab58e120722ccf2a4e8c93826b205/crates/tic-tac-toe/src/declared.rs#L307) | a resource → Turtle | hand-rolled | reads a source |
| [ikigai-tutorial `crates/spreadsheet/src/lib.rs:521`](https://github.com/ikigai-rs/ikigai-tutorial/blob/37a36c18787ab58e120722ccf2a4e8c93826b205/crates/spreadsheet/src/lib.rs#L521) | a formula → an s-expression, lossy by consent | hand-rolled | reads a source |
| [ikigai-cli `crates/ikigai-embedded/src/arrangement.rs:256`](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-embedded/src/arrangement.rs#L256) | a file → Turtle, planned on the root space outside any invocation | hand-rolled | reads a source (host side) |

That makes twelve copies in nine repositories. Eight hand-roll the step loop, and only the
three `inv.transrept` copies stream. The ikigai-cli copy plans on the root space, not in a
resolution chain, so a level's transreptor is not the one it uses.

There are two kinds of copy, and kernel negotiation helps both:

- **Serves `as`.** The endpoint answers its caller's `as` by converting its own answer.
  With kernel negotiation it answers in its own type and lets the kernel convert.
- **Reads a source.** The endpoint needs a source in a type it can parse. With kernel
  negotiation it issues `Source <src> as=<type>` and gets that type or a refusal. The
  converted face is then cached once, under `(src, as)`, and shared by every consumer.
  Today each composite keeps only its own final answer, so two consumers that both need
  the Turtle of one resource convert it twice.

## Recommendation

**Yes: the kernel should negotiate a `Source` answer on `as=`.** It already does this for
`Meta`. The inventory shows the loop copied twelve times with drifting details: whether
the copy streams, which scope it plans in, how it reports "no path", whether it handles
lossy steps. Introduce it as an opt-in kernel builder first, so default behavior does not
change, and make it the default in a later minor release.

### The rule

The kernel negotiates when all of the following hold:

1. The verb is `Source`, and the request carries an inline `as`.
2. The resolved endpoint **answered**. An error, a refusal included, is passed through as
   it is. **The kernel negotiates an answer, never an error.**
3. The answer is not a redirect (`Representation::redirect`) and not `Gone`. A redirect's
   JSON body describes a move, not the resource.
4. The answer's media-type essence differs from `as`. Parameters such as `charset` are
   ignored, and a structured suffix is not equality: `application/ld+json` is not
   `application/json`.
5. The request is not itself a step of a plan. Steps run with negotiation off, so a
   transreptor that answers the wrong type is an error, not a recursion.

### When it must NOT negotiate

- **An endpoint that serves several faces itself and refuses an unknown `as`** (ledger
  #2143, the kernel operations and the dynamic space's status). Its refusal is an error,
  so rule 2 lets it stand. The proof pins this for `urn:kernel:aliases`. This is why the
  rule keys on the answer, not on the contract: an endpoint that wants to own its faces
  says so by refusing, and nothing else is needed.
- **An endpoint that answers the type asked for.** Rule 4 makes this a no-op.
- **`Meta`**, which keeps its own path (below).
- **Any verb but `Source`.** A `Sink`'s or `Delete`'s answer is a report of an effect, and
  converting it would be guessing.
- **Plan steps** (rule 5).

### Composing with an endpoint that reads `as`

The endpoint's `Source` contract decides what the endpoint is handed:

- **It declares `as` as an input** (ikigai-engine's plan space, ikigai-name's document).
  The endpoint is invoked with `as`, and a mismatched answer is negotiated. Such an
  endpoint can delete its loop and keep its own faces.
- **It does not declare `as`, or has no contract.** The kernel resolves the request
  **without** `as` as an inner sub-request, then converts the result. The endpoint's
  single face is then cached once, under the bare key, and every face asked of it reuses
  that entry. Stripping `as` changes what an endpoint sees only if it reads `as` without
  declaring it. That gap is an audit of its own (follow-up 4).

### Caching under the requested type

The negotiated answer is stored under the request's own key, which includes `as`. It is a
composite of the answer and the steps, so the existing composite rules apply unchanged.
The expiry is the most restrictive of its parts. It is uncacheable if any part is, and a
caught refusal is never cached (ledger #3917). Caching the inner answer is the inner
request's business.

### Golden threads across the transreption

The negotiated answer hangs from:

- every thread of the answer it converted,
- every thread of each step,
- `BINDINGS_THREAD`, because the plan is a function of which transreptors are bound.
  Mounting a better transreptor, or unmounting the one used, has to drop the converted
  face. `Meta` hangs from the same thread for the same reason.

The proof shows the first point with today's API. A negotiator written as an ordinary
composite is cached under `(target, as)`, and cutting the CSV's thread drops the JSON. The
kernel gets this for free by issuing the inner answer and the steps as sub-requests of
the negotiated request, which is what `transrept_meta` already does.

### Errors when no path exists

Under negotiation, **no plan is a refusal**:
`InvalidArgument { name: "as", detail }`. The detail names the target, the type it answers
and the type asked for, and, where it can, the faces that *are* reachable. This matches
the ACCESSIBILITY.md rule that a refusal names what was asked and what is served.

This deliberately differs from `Meta`. A `Meta` request with no plan gets the canonical
Turtle, because a description in another type is still an honest description of the
endpoint. A `Source` caller who asked for JSON and was handed CSV has been handed a
different face, and most of the copies above already refuse in this case. Only
ikigai-diagram falls back. A caller who wants "whatever it is" omits `as`.

**A lossy-only plan** is refused the way `Meta` refuses it, naming the lossy steps and the
consent. How a `Source` caller spells consent without colliding with an endpoint's own
`lossy` argument is open (follow-up 3).

### Meta's existing path

`Meta` stays as it is: renderer first, then a plan from canonical Turtle, then
substitution with an honest `repr_type`. What it shares with the new path is the executor.
`transrept_meta`'s step loop and `Invocation::transrept` become one internal function
(follow-up 1), which also gives `Meta` the streaming steps it lacks today. The scope rule
is the same for both: plan and run in the scope where the endpoint was found, so a
transreptor bound on the endpoint's own level is the one used.

### Other properties the kernel path keeps

- **Depth.** Each step runs one level deeper than the negotiated request, and the depth it
  reaches counts toward the request's reach, as in `transrept_meta`.
- **Authority.** Steps run under the request's capability. A plan names only transreptors
  the chain can resolve. A step the capability cannot reach is refused (`Denied`), never
  skipped.
- **Trace.** Each step is a child span of the request, so `urn:kernel:explain` shows the
  conversion.
- **Streaming.** The executor streams to incremental steps and collects for the rest,
  under the run's byte budget.

## Proof

`crates/ikigai-core/tests/source_negotiation.rs`:

- `today_a_source_answer_ignores_as_even_with_a_plan_mounted` reproduces the claim.
- `a_negotiated_answer_is_cached_and_hangs_from_the_answers_threads` writes the
  negotiation as a composite: source the target, plan, `inv.transrept`. It shows that the
  composite answers the asked type, is cached under `(target, as)`, is served again
  without re-running the source, and is dropped when the source's thread is cut.
- `no_path_is_an_invalid_as_naming_both_types` shows the proposed refusal.
- `an_endpoint_that_refuses_its_as_keeps_its_refusal` pins the #2143 case: the refusal
  comes before any answer exists, so there is nothing to negotiate.

## Proposed follow-up items

1. **core: one transreption executor for Meta and `Invocation::transrept`.** Fold
   `transrept_meta`'s step loop into the executor behind `Invocation::transrept`, so both
   share scope, depth, trace and streaming behavior. This gives `Meta` incremental steps
   and leaves one loop to negotiate with.
2. **core: opt-in `Source` negotiation on `as=`.** A kernel builder that applies the rule
   in this note: answers only, not redirects, essence compare, steps unnegotiated, `as`
   stripped for endpoints that do not declare it, `BINDINGS_THREAD` dependency, and
   `InvalidArgument(as)` when no path exists. The tests in `source_negotiation.rs` become
   kernel tests.
3. **core: how a `Source` caller consents to a lossy plan.** Choose the argument name or
   request field for consent, so that it cannot collide with an endpoint's own
   arguments, and make it agree with `Meta`'s `lossy=allow`.
4. **core: `urn:kernel:validate` flags an endpoint that reads `as` without declaring it.**
   Negotiation strips `as` from such an endpoint's request. Find those endpoints before
   negotiation becomes the default.
5. **core: make `Source` negotiation the default.** Do this in a minor release after
   items 2 and 4, once the consumers below have moved. Record the change in the
   CHANGELOG as a behavior change for callers that pass an `as` nothing converts to.
6. **engine and name: drop the `as` loops.** Remove the loops in `plan_space.rs` (`eval`)
   and ikigai-name's `document.rs`, and answer in the endpoint's own type under
   negotiation.
7. **json, sparql, ui, diagram, toolbox, tutorial: read sources with `as=`.** Replace
   each "reads a source" copy in the inventory with one `Source <src> as=<type>`, or, until
   item 5, with `inv.transrept` instead of a hand-rolled loop.
8. **cli: plan the arrangement's conversion in a resolution chain.** `arrangement.rs`
   plans on the root space outside any invocation, so a level's transreptor is never the
   one it uses. Route it through the kernel's negotiation, or at least through
   `select_transreptor_in`.
