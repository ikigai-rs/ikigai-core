# Provenance labels: a request whose arguments came from an untrusted resource runs narrower

**Status:** design only (ledger #2389). Nothing here is built. What the kernel does today is
pinned by `crates/ikigai-core/tests/provenance_labels.rs`. The first increment (below) flips
that file's first test from "the Sink ran" to "the Sink was refused".

**Anchor types:** `ikigai_core::{Representation, Provenance, Capability, Invocation,
Description, ArgSpec}`.
**Companions:** `docs/design/sub-request-authority.md` (the confused deputy, and what core
said it cannot check), `docs/design/principal-capability.md` (a door's act versus a holder's
act), `docs/design/cache-ejection.md` (threads).
**Prior art:** Debenedetti et al., *Defeating Prompt Injections by Design* (CaMeL, 2025),
evaluated on AgentDojo. This note draws on what that paper describes, summarized below. It
does not re-derive the paper's results.

---

## The gap, reproduced

Reachability says what a caller CAN reach. It says nothing about misuse inside granted
authority. The test file sets up an agent endpoint holding `urn:cap:mail:send` and
`urn:cap:web:fetch`. The agent reads a page an attacker wrote and passes the address it found
there to a mail Sink by value. The Sink runs and sends to `attacker@evil.example`. Every step
stays within the grant, so no capability check can catch it (test 1).

Two halves of a remedy already exist, but nothing connects them:

- **The join is built.** The page's golden thread becomes a recorded dependency of the
  agent's answer. The kernel joins it there, the agent never names it, and the agent cannot
  leave it out (test 2). The kernel already knows where an answer came from, but only for the
  cache.
- **The "never trusted upward" algebra is built.** A capability exclusion is joined by every
  `attenuate` and `clamp`, and neither one ever removes it (test 3).
- **Missing: the link.** The Sink's invocation gets the agent's capability unchanged and an
  argument that is just bytes (test 4). It cannot see that the bytes came from the page.

`sub-request-authority.md` says that "taint-tracking strings through arbitrary Rust is not a
thing this kernel will do." This note keeps that position. The unit of tracking below is the
**invocation**, never the string. The kernel sees every invocation's sub-requests, and that
is all it needs to see.

## What CaMeL does, and what carries over

CaMeL splits an agent in two. A privileged planner writes a program from the user's request
alone and never sees tool output. A quarantined model parses untrusted data and cannot call
tools. A custom interpreter runs the program and attaches **capabilities** to every value:
its sources (which tool or user produced it) and its allowed readers. These are joined
through every operation that derives a value. Before each tool call, a **security policy**
reads the capabilities of the arguments and allows or refuses the call. For example, "the
recipient of `send_email` must not come from untrusted sources unless it is already a known
contact."

What carries over:

- **Labels on values, joined through derivation, and checked at the privileged call.** This
  is the item's request.
- **Policy at the sink, not at the source.** Reading an untrusted page is fine. What gets
  refused is untrusted data driving a privileged action.
- **Explicit endorsement.** A value leaves a label behind only through something the policy
  names, never through the code that holds the value.

What does not carry over: CaMeL's interpreter sees every value, so it labels per value. In
ikigai an endpoint is arbitrary Rust, so the kernel sees only the boundaries: what an
invocation read, and what it issued. Labels are therefore **per invocation**: an
over-approximation that stays sound (see "Precision" below).

## Decisions

### 1. A label is an IRI carried on the representation, beside its threads

```rust
// sketch, not built
pub struct Label(Iri);              // e.g. urn:label:untrusted, urn:label:web
impl Representation { pub fn labels(&self) -> &BTreeSet<Label>; pub fn labeled(self, l) -> Self; }
pub struct Provenance { pub expiry, pub threads, pub labels: BTreeSet<Label> }
```

A label is a **vocabulary term on the representation**, `ik:label`, with a class `ik:Label`
in ikigai-vocab. It is not a capability exclusion, for three reasons:

- An exclusion is read **by the module that spells it** (`allows` never grants one, and the
  floor's family form treats it as present). A label has to be checked by the kernel for
  every action, whoever wrote the action.
- An exclusion lives in a family's grammar (`urn:cap:<family>:<qualifier>:-<rule>`).
  `is_deny_scope` recognizes the `-` only in the second or third segment, so one label would
  need a different spelling in every family that might receive it. Test 3 had to use
  `urn:cap:mail:send:-label:untrusted` for this reason. A label is about where a value came
  from, and that does not depend on which family receives it.
- A representation already carries the other thing that joins through a derivation: its
  threads. A label follows the same path and is joined at the same lines in the kernel,
  where `dependency_threads()` and `incoming.threads` are folded into the answer.

The capability algebra still supplies the rules for labels, applied to the label set:
**labels only accumulate**, as exclusions do. No operation available to an endpoint removes
one.

### 2. Where labels are joined

These are the places where the kernel already joins threads, plus one new place:

1. **An answer's labels** = the labels the endpoint added itself (adding is narrowing, so it
   is always allowed) ∪ the labels of every sub-request answer it read ∪ the labels of the
   incoming pipe `Provenance` ∪ the labels of every **by-reference argument** the kernel
   settled for it.
2. **A cached entry stores its labels**, as it stores its threads. A cache hit returns them,
   so "it was cached" never removes a label. Labels are a function of what was read, and
   that is already part of the entry's identity.
3. **New: the issuer's labels so far travel with every sub-request it issues.** When an
   invocation issues a request, the kernel attaches the union of everything that invocation
   has read up to that moment. The kernel holds this set already, as recorded dependencies.
   This is the "sticky header" of the formalism, driven by data instead of by authority.
4. **Across a wire**, labels a peer carries are joined with the label the door assigns to
   that peer: a union, as `clamp` keeps exclusions from either side. A peer can add labels
   but cannot shed the door's.

**Where labels come from.** Labeling a source is a **host's act**, in the same trust class
as minting a principal: a mount, door or space is declared to label what it answers. For
example, the HTTP fetch door is declared `urn:label:untrusted`. An endpoint may also label
its own answer. Neither one can remove a label.

### 3. How a policy names a label: in the contract, enforced at the kernel's floor

```rust
// sketch, not built
ActionSpec::new(Verb::Sink)
    .input(ArgSpec::new("to").refuses("urn:label:untrusted"))   // per argument
    .refuses("urn:label:untrusted")                             // the whole request
```

At dispatch, at the same floor where `requires` is checked, the kernel refuses a request
when a refused label is present:

- **per argument**, for a by-reference argument, using the labels of the referent the kernel
  settled. This is exact.
- **per request**, for a by-value argument, using the issuer's label set from point 2.3. The
  kernel cannot tell which inline bytes came from where, so every inline argument carries
  the issuer's whole set.

A refusal is `Error::Denied`. It names the argument, the label, and the thread the label
came from (the cache already records that thread), so the person reading it can see "`to`
came from `urn:t:web:page`". Error text follows ACCESSIBILITY.md.

A host can also add refusals to a contract it did not write, through its policy (the
gatekeeper). The host can tighten a contract this way but never loosen it.

### 4. A declared sanitizer is the host's endorsement, not the endpoint's

"No endpoint can clear a label it received" holds without exception for endpoints. A
**sanitizer** is a host declaration:

```text
endorse urn:label:untrusted  by  urn:t:addressbook:lookup
```

This reads: "an answer from `urn:t:addressbook:lookup` does not carry `urn:label:untrusted`,
even if its inputs did." The kernel applies it when it joins that endpoint's answer, and
removes **only** that label: every other label still passes through. The sanitizer's own
code cannot invoke this. The host's table does it, in the same way that `clamp` is a door's
operation rather than a holder's. Matching CaMeL's example, a good sanitizer narrows the
data to a closed set: the lookup answers only with addresses already in the user's address
book.

## The worked case

The page answers with `urn:label:untrusted`, because the host labeled the fetch door. The
mail Sink's `to` refuses that label.

| Arrangement | What the kernel does |
|---|---|
| The agent sources the page, then Sinks `to` inline | The agent's label set holds `untrusted` once it has read the page. It travels with the Sink request, and the request is **Denied** (test 1 flips). |
| The agent Sinks `to` as a reference to the page | The referent is labeled, so the request is **Denied** for `to`, exactly. |
| The agent passes the page through `urn:t:addressbook:lookup`, then Sinks the result | The lookup's answer is endorsed: `untrusted` is removed and the others pass through. If the agent read **only** the lookup's answer, the Sink is **allowed**. |
| The same, but the agent also read the page directly | The agent's own label set still holds `untrusted` from that direct read, and the request is **Denied**. Splitting the work is the fix (see "Precision"). |

The last row is the cost of per-invocation labeling, and it is the intended behavior.

## Precision: why per invocation is the right grain

Per-invocation labeling over-approximates: a value counts as tainted if anything its
invocation read was tainted. The over-approximation is sound, and it can be recovered in the
same way CaMeL's planner and quarantined model are split:

- An agent that **reads** untrusted data does it in a sub-invocation whose answer is a
  label-carrying representation.
- It passes that answer **by reference**, or through an endorsed sanitizer, to a privileged
  action.
- The orchestrating invocation that issues the privileged call never reads the untrusted
  bytes itself.

That orchestrator is CaMeL's privileged planner, and the structure is enforced by the label
rather than by convention.

## What it costs and what it touches

- `Representation` and `Provenance` gain a label set, and cached entries store it. The wire
  and Turtle faces need a `ik:label` term (ikigai-vocab).
- `Invocation`'s recorded dependencies gain the labels of what it read. The data structure
  exists already, next to `dep_threads`.
- `ArgSpec`, `ActionSpec` and `Description` gain `refuses`. The floor check sits beside
  `requires`. The manifold (`urn:kernel:actions`) and `urn:kernel:validate` must report the
  same refusals, as they report `requires`, so that what is offered matches what is
  admitted.
- The host gets a labeling declaration for mounts and an endorsement table. Both are host
  acts, and neither is reachable by an endpoint.
- The cache key does not change: labels are a function of the inputs that are already part
  of it.

## First increment (proposed)

1. `Representation::labeled` / `labels`, joined at the thread join, and stored in the cache.
2. The issuer's label set travels with each sub-request.
3. `ArgSpec::refuses` / `ActionSpec::refuses` at the floor, refusing with `Denied`.
4. A host endorsement table with one entry kind.
5. Flip `provenance_labels.rs` test 1, and add the four rows of the worked case as tests.

## Open questions

- **Label vocabulary.** Should there be one global `urn:label:untrusted`, or labels that name
  their origin (`urn:label:origin:<door>`), with policies matching a family by `*` as
  scopes do? Origin-naming labels are more useful for a refusal message, and allow
  "untrusted unless from my own mailbox".
- **Readers.** CaMeL also tracks who may *read* a value. ikigai's principal-scoped content
  (ledger #2965) covers part of that. Whether labels should carry readers is left out of
  this note.
- **The formal side.** "Labels only accumulate along a chain, except at a declared
  endorsement" is a monotonicity property of the same shape as the scope-floor certificate's
  hypotheses (`docs/formal/`). It should be stated there once the increment is built.
