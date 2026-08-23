# Resolution observability: capability, model-tier, and a trace that crosses the wire

**Status:** design / brief. Not yet built.
**Anchor type:** `ikigai_core::TraceEvent` (kernel.rs). **Spans:** ikigai-core + the
ikigai-cli workspace (resolve, engine, wire, ipc/quic transports). **Owner:** hub
(cross-repo arc; not a single-satellite repo — see *Logistics*).

## Why this, and why now

ikigai's value is Reed-shaped: composition is arbitrary-arity (a pipeline chains
*k* stages, a transclusion pulls *k* sources, a join spans *k* graphs), so each
mounted capability doubles the space of subsets it can join. Raw potential is only
*realized* as `potential × findability × trustability`. Selection (manifold →
`urn:agent:select` → `urn:kernel:validate`) is the **finder** and is shipped.
**Observability is the other converter** — it is what lets you operate high in the
arity space, because a deep composition across mounts, grants, and model tiers is
only worth building if you can see how it resolved when it breaks. Without it you
self-limit to shallow, debuggable compositions and collapse back toward Metcalfe.

Today the `trace` command reconstructs the execution tree **within one kernel**
(target chain = mounts, span/parent = composition + fan-out, `cache_hit`, timing,
worker) — genuinely good, but it stops exactly at the boundaries that make the
curve exceed one head:

1. **No capability on the event** — attenuation across a grant boundary is invisible.
2. **No model/tier** — `urn:llm:select`'s local-vs-frontier choice is only inferable.
3. **The wire tracer is a no-op** — `set_tracer` on a wire resolver does nothing
   ("a wire resolver can't yet trace the remote kernel"), so any resolution that
   crosses `--connect`, an IPC/QUIC mount, or a cross-process grant **truncates at
   the boundary**.

## Definition of done (the acceptance test)

Trigger the fullest resolution available — one that crosses a mount, a federation,
a grant boundary, and a model-tier escalation — and get back **one stitched trace
tree** in which every node names: `target`, the **capability** it ran under, the
**cache** state, the **model/tier** (where an LLM served it), and the worker —
including nodes that executed in a *remote* kernel. Not "did it work"; "how did it
work," across the boundary.

## Current shape (verified)

- `TraceEvent { target, thread, started, ended, cache_hit, span, parent, capability,
  notes }` (`ikigai-core/crates/ikigai-core/src/kernel.rs`). `(span, parent)` edges
  rebuild the tree. Recorded only while a `Tracer` is installed (off the hot path
  otherwise). **`capability` (Phase 1) and `notes` have since landed** — the field
  list above was written before them; Phase 2's annotation channel became `notes`,
  populated by `Invocation::trace_note` and, now, by capability denials.
- `Kernel::set_tracer` / `clear_tracer`; `Tracer::record(&self, TraceEvent)`.
- `Resolver` (ikigai-resolve) forwards `set_tracer` to the in-process kernel;
  **the wire resolver's `set_tracer` is a documented no-op** — this is gap #3.
- `run_trace` + `TraceCollector` + `render_trace_tree` (ikigai-engine) install a
  collector for one resolution and render the tree.

## Plan (checkpointed, smallest-first)

### Phase 0 — confirm the seams (no code; a short design-confirm note)
Read and confirm before touching anything:
- **`Provenance`** (core; already imported in ikigai-resolve) — is this the right
  channel for an endpoint to stamp `model`/`tier`, lifted into the event by the
  kernel? Preferred over widening the endpoint return type.
- The **wire request frame** (ikigai-wire) — where an optional trace-context field
  attaches, and how the response frame can carry returned remote spans.
- The **mount/remote resolve path** in ikigai-resolve — where a wire resolver would
  attach context out and collect spans back.
Deliverable: a paragraph in this doc's PR confirming each, or noting a better seam.

### Phase 1 — capability on the event (core; self-contained, ships value alone)
- Add `capability: Option<String>` to `TraceEvent` (a compact scope summary or the
  fingerprint the kernel **already computes for the cache key** — cheap to thread).
- Populate it where the kernel records the event; render it in the tree.
- Now grant attenuation is visible *within* a kernel — worth having on its own.
- **Core API change → core version bump.** Merge the bump PR + verify origin/main,
  then Brian publishes; ikigai-cli picks it up via its caret pin.
- *Checkpoint:* PR + green CI (checked explicitly), then hand off to publish.

### Phase 2 — model/tier legibility (depends on Phase 1 + an LLM resolver)
- Add a small annotation channel: either `attrs: Vec<(String, String)>` on
  `TraceEvent` (OpenTelemetry-style span attributes; `model`/`tier` the first keys)
  **or** lift a whitelisted subset of `Provenance` into the event (Phase 0 decides).
- The LLM resolver ([[urn:llm:ask]]) stamps `model`/`tier`; the tree shows it.
- NOTE: `urn:llm:*` may not be built yet — if absent, Phase 2 is a no-op stub and
  should be deferred rather than blocking Phase 3. Keep it independent.

### Phase 3 — the keystone: propagate trace context over the wire
Turn the no-op into a real stitch (IPC first, QUIC identical shape):
- A **trace context** `{ trace_id, parent_span }` rides on the request in the wire
  frame (W3C `traceparent`-style), attached by the wire resolver **only when a
  tracer is active** (preserve the off-hot-path discipline).
- The remote kernel, seeing a context, records into a tracer tagged with that
  `trace_id`, parenting its root at the incoming `parent_span`.
- Remote spans return to the caller (inline in the response frame, or streamed) and
  the caller's `TraceCollector` **merges** them into its tree.
- **Span identity across kernels:** spans are unique only within a kernel today. Add
  an origin/kernel label and stitch on `(origin, span)`, or re-base remote spans
  into the caller's space at merge. Decide in Phase 0/3; keep the merge total.
- *Checkpoint:* demo `ikigai --connect <sock> trace <iri>` returning a tree whose
  leaves executed in the remote `ikigai-dev` and are labeled with their capability.

### Phase 4 — MCP grant boundary (optional, later)
The MCP bridge synthesizes a root span per `tools/call` so a cross-process grant
crossing appears in a trace. Lower priority: external MCP clients won't send a
context, so this is server-side synthesis, not propagation.

## Phase 3 seams — confirmed (Phase 0 read, 2026-07-06)

Read: `ikigai-wire/src/lib.rs`, `ikigai-resolve/src/lib.rs`, `ikigai-ipc/src/lib.rs`.

**Prerequisite (core, one line).** `TraceEvent` derives only `Clone, Debug` — **no
serde** — so spans cannot cross the wire as-is. Phase 3 begins by adding
`Serialize, Deserialize` to `TraceEvent`; every field already serializes (`Time`
derives serde, and `Option<Vec<String>>` is trivial). A core bump + publish gates
the wire work.

**Wire frame (`ikigai-wire`).** Postcard, non-self-describing (client+server ship
together), `PROTOCOL_VERSION` bumps **2 → 3**. Add *appended* variants so existing
discriminants are unchanged:
- client→server: `Call::IssueTraced(Request, Capability, TraceContext)` where
  `TraceContext { trace_id: u64, parent_span: u64 }`. (`IssueAs` is the
  cap-carrying form; the traced form extends it; only a traced session sends it.)
- server→client: `Reply::ResolvedTraced(Representation, CacheStatus, Vec<TraceEvent>)`
  — the representation plus the spans the server recorded for this call.

**Server (`ikigai-ipc::dispatch` / `handle_connection`).** On `IssueTraced`: install
a `TraceCollector` (`Kernel::set_tracer`), seed the span counter so recorded spans
carry `ctx.trace_id` and parent under `ctx.parent_span`, run `issue_as`, drain the
collector, `clear_tracer`, return `ResolvedTraced`. `ikigai-quic` shares the same
`encode`/`decode` and a one-stream-per-call model → identical dispatch change.

**Client (`ikigai-resolve` wire `Resolver` — the current no-op `set_tracer`).** Make
it real: store the tracer; when one is installed, send `IssueTraced` with the
active context and, on `ResolvedTraced`, forward each returned `TraceEvent` to the
stored tracer.

**Do the easy, high-value half first:**
- **3a — whole-session `--connect` trace.** The entire resolution runs on the remote
  kernel; the client only forwards. All spans are in ONE (remote) namespace — no
  re-basing, render as-is. This alone satisfies "trace a resolution that crossed the
  wire" and turns the no-op real. Smallest shippable Phase 3.
- **3b — mount-stitch.** A *local* kernel resolving through a wire-mounted sub-space
  records local spans while the remote records its own; the local mount-point span
  must parent the remote root, and remote span ids must be **re-based** into the
  caller's space (offset per mount). Depends on the mount-over-wire Space
  (multi-connect / reverse-mount). Follow-on.

**Design risk — the tracer is process-global.** `Kernel`'s tracer is a single
`Mutex<Option<Arc<dyn Tracer>>>`, and `handle_connection` spawns a thread per
connection, so two *concurrent* traced calls on one server would interleave into
one collector. Fine for the one-shot interactive `trace`; before concurrent traced
use, either serialize traced calls, filter recorded events by `trace_id`, or thread
a per-call context through invocation instead of a global tracer. Note, don't fix now.

**MCP boundary (Phase 4, unchanged).** External MCP clients won't send a
`TraceContext`, so that boundary is server-side root-span *synthesis*, not
propagation — out of Phase 3 scope.

## Capability denials — the event that names something which never RAN

**Status: built.** Found by ikigai-log T2, 2026-08-23.

Every event above describes an invocation that HAPPENED. A capability denial is the
one fact on this surface that describes one that did not, and it was invisible.

The kernel enforces a bound endpoint's declared `requires` **before dispatch** —
that is the whole point of "declared = enforced" (0.1.49), and it means the endpoint
is never entered. So `Tracer` never saw a denial, `trace_note` could not reach it,
and the refusal reached **no in-kernel observer at all**. For ikigai-log that bites
exactly: `log:CapabilityDenied` is an always-land class *because a refused authority
is a security fact*, and `urn:log:write` is the one action that cannot write it — a
denied write was never entered. **The one action that could record the refusal is
the one that was refused.**

The kernel now records a `TraceEvent` on denial, noted `(DENIED_NOTE, "<scope>")`
with the scope the caller lacked. `target` and `capability` were already the two
facts such an entry needs, and `notes` already existed — so **no struct change and
no postcard layout bump**, which matters because `TraceEvent` crosses the wire and
adding a field forces a protocol-version bump on every host shipping it.
`started`/`ended` are `None` and `cache_hit` is `false`: the honest reading of a
request that never ran. Both enforcement points report — bound endpoints
(`unsatisfied_scope`) and the `urn:kernel:*` builtins (`issue_kernel`'s gate).

**Deliberate asymmetry:** a *successful* `urn:kernel:*` builtin records no event at
all — that arm returns before the trace path and is likewise absent from the
constraint window, because these are introspection, not throughput. An observer
therefore sees denials from that namespace and never successes. The refusal is the
fact worth keeping.

### ★ The coupling this creates, and the API it argues for

A `TraceEvent` lands only into a `TraceScope`, and `Kernel::global_scope()`
short-circuits on the `tracing` atomic that `set_tracer` flips. So **a host that
installs a `Tracer` in order to see denials thereby builds an event for every
resolution** and must filter in its own `record()`. "Capability denials always land"
costs "trace everything".

The sharp form: the cost is **an allocation per resolution even at `error` level,
where the log writes nothing** — you pay to build an event you then discard.

That is the concrete argument for a future opt-in reporter trait alongside `Tracer`,
letting a host observe *only* refusals without paying per resolution. It was
deliberately NOT built here: it is new public surface in core, and designing it
before its first consumer (ikigai-log T5) has used this seam risks the wrong shape.
Adding it later is additive and non-breaking. **This belongs in the T5 brief, where
it changes a decision.**

## Logistics & constitution

- **Hub work, not a satellite repo:** the arc spans ikigai-core (`TraceEvent`) and
  the ikigai-cli workspace (resolve/engine/wire/ipc/quic). Core gates cli across the
  release boundary — Phase 1 must land **and publish** before Phase 3 consumes the
  new fields. Local `paths` redirect during dev; **never commit it**.
- Every change via PR with **green CI, verdict checked explicitly**. Stage named
  files only. Merge the version-bump PR + verify origin/main **before** Brian
  publishes (publishing is always Brian's action).
- Sibling arcs that will also want a wire-carried field on the request:
  capability-on-the-wire (transports TODO) — coordinate the frame change once.

## Payoff

With capability on the event, every identity-bound-grant action (e.g. a Tier-2
social post acting under a DID-scoped grant) becomes auditable as "DID X triggered
this resolution under cap Y against these sources." Observability and identity-bound
caps then compose into a public-facing, fully auditable capability system — the
second converter that lets you actually spend the Reed exponent.
