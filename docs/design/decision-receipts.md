# Decision receipts: the kernel's verdict, Deny as well as Allow

**Status:** proposed (ledger #4509). A design spike: this note and a test-only prototype,
`crates/ikigai-core/tests/decision_receipts.rs`. Nothing in `src/` changes. The decisions
for Brian are at the end.

**Source.** An agentic-attestation draft spec, whose best idea is that a receipt must carry
the ENGINE's verdict, produced in the same call that decides, rather than a payload the
caller chose.

---

## Why the kernel, and what it emits today

The kernel is ikigai's single decision point. Every request is resolved and checked against
the capability floor there ("declared = enforced", `Kernel::issue_with` in `kernel.rs`):
every `requires` the resolved endpoint declares for the verb is checked with the predicate
that selection and `urn:kernel:validate` use. The floor runs before the endpoint's cache
entry is looked up, so a cache hit is a decision too.

A receipt written anywhere else is a claim about the kernel, not the kernel's own record. A
module that logs "I allowed this" logs what it chose to log, and a denied request never
reaches the module at all (`trace-observability.md`, "Capability denials").

What core emits today is the trace. A denial becomes a `TraceEvent` noted
`(DENIED_NOTE, <first lacking scope>)`. The first test in the prototype,
`today_the_denial_trace_cannot_be_re_evaluated`, shows why that cannot serve as a receipt:

- it names the target but **no verb and no arguments**, so it does not identify the request;
- it names **only the first** scope the caller lacked, and nothing of the requirement set
  that decided it, so nobody can re-evaluate the verdict from it;
- it names no kernel version and, for a denial, no instant;
- an Allow event carries no decision facts at all, only that the request ran;
- it reaches an observer only through a `Tracer`, which builds an event for every resolution,
  and a tracer is allowed to drop events.

## The receipt

One receipt per floor decision, Allow or Deny, with these fields. All digests are BLAKE3,
domain-separated and length-prefixed with the same helpers as `hashing.rs`.

| Field | What it commits to |
|---|---|
| `kernel` | The `ikigai-core` version that decided. |
| `floor_rule` | The predicate's name, `ikigai.floor.v1`: the `Floor` table (which requirements apply to a verb the endpoint did not declare) and `cap_satisfies` (exact scope, `*` family, deny-shaped exclusions). A crate version says which code ran. This field says which rule a verifier must apply, and it changes only when the rule does. |
| `request` | `ikigai.receipt.request.v0`: the verb, the canonical target, then each argument in name order: its name, its kind, and either its value in clear (only `as=`) or a **keyed commitment** to the value (all other arguments; see "What a receipt must never contain"). |
| `capability` | The cache's capability fingerprint (`capability_key` in `kernel.rs`), same encoding, **untruncated**. The cache keeps 64 bits, which is enough to partition, but not enough for a commitment a third party relies on. The receipt keeps all 256, and the cache's `u64` is their first 8 bytes. |
| `grant_state` | `ikigai.receipt.grant.v0`: what decided the verdict. The endpoint the chain resolved to (its canonical IRI), the verb, the floor's requirement set for that verb (sorted), and the host grant the request ran under, if it was issued with `issue_granted` (the grant's prefix and scopes). |
| `at` | The kernel's operational time (`Kernel::now`) when it decided. A kernel with no clock cannot bind a sink (decision 7), so this field is never absent in a real receipt. |
| `pinned` | The temporal corridor's pinned instant (`Invocation::pinned_now`'s source) when the request was resolved inside one, else absent: the world time the decision was made *as of*, distinct from when it was made. |
| `prev` | The hash of the previous receipt on this kernel's chain; all zeros for the first. |
| `verdict` | `Allow`, or `Deny` with **every** lacking scope (the `Denial::scopes` list, not only the first), or `Deny(concealed)` when an existence-sensitive door answered `Unresolved`. The caller sees "not found", and the receipt records the refusal. |

The receipt does not commit to the answer. An Allow receipt says the kernel let the request
through, not what came back (see "What a receipt must never contain").

### Built where it is decided

The design puts receipt construction inside the floor check, at the one site that already
builds the `Denial` (and its Allow branch), using values the kernel already holds there: the
request, the capability, the resolved endpoint's memoized description, and the grant. No
caller can supply or alter a verdict, because no caller is on that path. The prototype stands
outside the kernel. It reads the verdict from the call's `Err(Denied)` and checks it against
the trace's denial note. That is as close as public API gets, and it is why the prototype is
test-only.

## Where it is written

The receipt goes to a **sink the host binds**, a new opt-in trait beside `Tracer`. This is
the "reporter that observes only refusals" that `trace-observability.md` anticipated, widened
to cover every decision:

```rust
pub trait DecisionSink: Send + Sync {
    /// Called once per floor decision, before the answer or refusal is returned.
    /// `Err` means the sink cannot take it, and the kernel refuses the request.
    fn append(&self, receipt: &Receipt) -> Result<(), SinkFull>;
}
```

It is not a `Tracer`: a tracer is best-effort, observes everything, and may filter or drop
events. A sink is a write-ahead obligation. **ikigai-log** is where receipts land: a new
always-land entry class (proposed name `log:KernelDecision`), next to `log:CapabilityDenied`,
written by a sink that ikigai-log provides. Core names no log format and depends on no log
crate: the trait is core's, and the ikigai-log line is ikigai-log's.

## Cost per request, and how it is bounded

Sampling is not acceptable for a Deny: a sampled denial log is exactly the log an attacker
probing scopes wants. The design does not sample Allows either, because a chain with gaps
cannot show that nothing was dropped. Instead:

- **The per-request work is fixed.** One keyed hash per argument (BLAKE3, a few hundred
  nanoseconds for small values), one request digest and one receipt hash. The capability
  digest is the hash the cache key already computes, kept at full width. The grant-state
  digest is **memoized** with the endpoint's description memo, per (endpoint, verb, grant
  table). It is computed once per binding and forgotten when `urn:kernel:bindings` is cut, as
  the description memo is today. There is no allocation beyond the receipt itself.
- **Argument values are hashed, never copied.** A large inline body argument costs one hash
  pass over bytes the request already holds.
- **The chain is one 32-byte critical section.** Each kernel has one chain head behind a
  mutex. The receipt is built outside the lock and only the `prev` link and the hash are taken
  inside it.
- **Signatures are per batch, not per receipt** (see "Envelope").
- **The bound is backpressure, never loss.** The sink has a bounded buffer. When it is full,
  `append` returns `Err` and the kernel **fails closed**: it refuses the request with the
  existing `Error::RetryAfter` and dispatches nothing. The sink keeps one slot in reserve for
  a single "buffer full since receipt X" record, so the outage itself is on the chain. A host
  that cannot afford this does not bind a sink. A host that binds one has said that an
  unrecorded decision is worse than a refused request.
- **Deny floods are bounded upstream.** What limits a flood of denials is the door's
  admission control per principal, ahead of the kernel, not a receipt log that goes quiet.

## What a receipt must never contain

- **A secret argument's value.** Core has no "secret" marker on `ArgSpec`, and none is
  proposed, because a marker someone forgets is a leak. Instead **every** argument value
  except `as=` is committed by a BLAKE3 keyed hash under a **host-held key**, never put in
  the receipt as a plain hash. A plain hash of a PIN or a short token is a dictionary away
  from the value. A keyed one is not, and the host can still open it to a party it chooses by
  disclosing the request and the key. `ArgRef::Content` addresses are committed the same way,
  because the address of a known secret confirms it. Argument names, the verb and the target
  are in the commitment's clear input and are disclosed with the receipt. A target IRI that
  carries a secret is a door's design error, not a receipt's to hide.
- **A read's bytes, or their digest.** The receipt is written at the floor, before dispatch,
  so it never sees the answer. This is deliberate: an Allow receipt that committed to the
  answer's content address would let anyone holding a guess confirm a secret read. Proving
  what was answered is ikigai-sign's job on the answer, under the reader's own authority.
- **A capability's scopes in clear.** The receipt holds their digest. The host discloses the
  set to a verifier it chooses. (The principal scope is in the set. ikigai-log's `principal=`
  column already records it, so the log line may carry it, but the receipt itself does not.)

## Envelope

A receipt has no format of its own. It has two envelopes, and both already exist:

1. **The record: an ikigai-log line plus an ikigai-sign signature graph over a batch.** Each
   receipt is one ikigai-log entry of class `log:KernelDecision`, with the fields above as
   the entry's columns (digests in lowercase hex). The sink closes a **batch** every N
   receipts or T milliseconds, whichever comes first, and ikigai-sign signs a graph whose
   subject is the batch: its first and last receipt hashes, its count, and the chain head.
   Because the chain links every receipt, signing the head signs every receipt before it.
   Verifying a single receipt means walking from it to a signed head, and the batch bound
   limits that walk to N.
2. **The export: a VC 2.0 credential through ikigai-credential.** When one decision must be
   handed to a third party outside the log (an auditor, a counterparty), the host issues a
   credential whose subject is the receipt's fields and whose evidence is the signed batch
   head and the chain path to it. This is an export of the record, not a second record.

The shapes inside those envelopes (column spellings, the graph's predicates) belong to
ikigai-log, ikigai-sign and ikigai-credential, and are proposed to them in the task list.

## How a verifier re-evaluates

The verifier is handed the chain (or a segment of it from a signed head) and, for each
receipt it wants to check, a **disclosure** from the host: the capability's scope set and the
grant state (endpoint, verb, requirements, grant). It then:

1. **Checks the chain:** each `prev` equals the hash of the receipt before it, and the
   segment ends at a head ikigai-sign signed.
2. **Checks the disclosure against the receipt:** the capability digest of the disclosed set,
   and the grant-state digest of the disclosed state, each equal the receipt's.
3. **Re-evaluates with the kernel's own predicate,** the one `floor_rule` names. The verifier
   builds a `Description` that declares exactly the disclosed requirements for the verb and
   asks `Description::unsatisfied_scopes(verb, &capability)`. If nothing is lacking the
   verdict is Allow. Otherwise it is a Deny with exactly that list, and it must equal the
   receipt's. This is public API today, so a verifier links `ikigai-core` at the receipt's
   `kernel` version (or any later one whose `floor_rule` is unchanged) and needs no code of
   its own.
4. **Optionally checks the request,** if the host also discloses the request and its
   commitment key: recompute `request` and compare.

The prototype shows each failure mode: a host that understates the requirement set
(`GrantStateMismatch`), one that discloses a broader capability (`CapabilityMismatch`), one
that rewrites a Deny as an Allow and re-links the chain (`VerdictDisagrees`, caught by
re-evaluation even before the signature), and one that drops a Deny (`BrokenChain`).

What re-evaluation does **not** cover: whether the disclosed requirement set was the one the
endpoint really declared. The grant-state digest binds the host to one answer at decision
time. Checking it against the endpoint's published `urn:kernel:actions` manifold, or its
signed module manifest, is the verifier's next step, and is outside the kernel.

## What a receipt does not cover

- **A module's own refusal.** A door that checks a finer ACL itself and returns
  `Error::Denied` was allowed by the floor. The kernel's receipt says Allow, truthfully. The
  module's refusal is the module's verdict and is not signed as the kernel's (see decision 3).
- **Sub-requests are decisions too.** Each `issue`, `issue_attenuated` and `issue_granted`
  from inside an endpoint passes the floor and takes its own receipt under the capability it
  ran with. Under a grant, the grant's entry is in `grant_state`, so a verifier sees that the
  authority came from the host's delegation and not from the caller.
- **Builtins.** `urn:kernel:*` operations pass their own gate (`issue_kernel`) and take
  receipts the same way. Their requirement set is the builtin's scope.

## Decisions for Brian

1. **Allow receipts on every request, or Deny-only?** Recommended: every decision. A chain
   with only Denies cannot show that an Allow was not hidden, and the per-request cost is a
   fixed few hashes. A Deny-only mode would halve nothing that matters on the read path,
   where cache hits dominate and the hashes are already the cheap part.
2. **Fail closed when the sink is full?** Recommended: yes, with `RetryAfter`. The
   alternative, dropping and counting, is the sampling this task rules out.
3. **Module refusals: a second receipt class, or out of scope?** Recommended: out of scope
   for the kernel's receipt. A module that wants its refusals attested writes them through
   its own sink under its own key. The kernel signs only what the kernel decided.
4. **One chain per kernel, or one per worker thread merged at batch close?** Recommended:
   one per kernel, until a measurement shows the 32-byte critical section is contended.
5. **The commitment key's lifetime.** Recommended: one per batch, derived from a host secret
   and the batch number, so disclosing one batch's key opens that batch alone.
6. **The capability digest at full width in core.** It needs one new crate-private function
   (the existing `capability_key` returning all 32 bytes, with the `u64` taken from it), so
   the receipt and the cache cannot drift apart. Recommended: yes.
7. **A sink requires a clock.** Recommended: `with_decision_sink` on a kernel without
   `with_clock` is refused, because a receipt without an instant cannot be ordered against
   anything outside its own chain.

## Task list

1. Core: `DecisionSink` trait, `Receipt` struct and `Kernel::with_decision_sink` (opt-in,
   additive), built at the floor and `issue_kernel`'s gate. Full-width capability digest.
   Grant-state digest memoized with the description memo.
2. Core: `ikigai.floor.v1` written down as a normative section of this note (the `Floor`
   table and `cap_satisfies`), plus shared vectors in `tests/vectors/` so a verifier in
   another language can re-evaluate.
3. Core: `Deny(concealed)` for existence-sensitive doors, and receipts for sub-requests and
   granted sub-requests, each with a test.
4. ikigai-log: the `log:KernelDecision` entry class and its columns, and a sink with the
   batch close (N, T).
5. ikigai-sign: the batch-head signature graph, and a chain-segment verifier.
6. ikigai-credential: the VC 2.0 export of one receipt with its chain path.
7. A verifier crate or example that runs steps 1 to 4 of "How a verifier re-evaluates" from a
   log segment and a disclosure.
8. Measure: the per-request overhead on the shared-cache baseline (`tests/shared_cache_baseline.rs`)
   with a sink bound, before decision 4 is revisited.
