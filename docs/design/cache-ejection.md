# Ejecting the representation cache — design

**Status:** design only, 2026-09-12. Nothing here is built. Written alongside the
cache extraction and the lost-cut fix (core 0.1.70) because the ejection question is a
*constraint* on those two, and discovering it afterwards would be discovering it too
late. Companion to `ikigai_core::cache`; the code is the spec where they differ.

---

## The ask

> I want to be able to someday eject the cache into a serialized form. That could be
> useful as a way of pre-calculating things and letting another instance benefit from
> that work. — Brian, 2026-09-12

A derived graph, a transreption, a SHACL report over a fixed shapes file: expensive,
deterministic, and computed identically on every machine that asks. Computing one on a
build host and shipping the answer is worth real time on the edge and in CI.

## The part that is not the problem

Serialisation. `Representation`, `ReprType`, `Expiry`, `Thread` and `RequestId` all
derive `serde` already, so `CacheEntry` and `CacheKey` derive it for free — they do, as
of 0.1.70, precisely so the door stays open at no cost. A postcard or JSON bundle of
entries is an afternoon.

**The derive is not permission to import one.** What follows is why.

## The part that is the problem: validity across processes

### 1. Golden-thread generations are per-process counters

An entry pins each thread it depends on to the generation that thread held when the
entry was stored — `("urn:data:books", 6)`. The number is a count of cuts *this
process has seen*. Another instance's 6 is not this instance's 6, and neither number
says anything about whether the underlying state has changed. Import an entry pinned at
6 into a kernel whose counter for that thread stands at 0 and it looks valid forever;
import it into one standing at 9 and it looks stale immediately. Both readings are
noise.

Three ways out, in increasing order of honesty:

- **Import only thread-free entries.** `Expiry::Never` with an empty thread set is a
  pure function of its inputs: nothing to reconcile, because nothing can invalidate it
  short of the code changing (see §3). This is the honest first tranche and covers the
  motivating case — pre-computing derived graphs and transreptions.
- **Re-validate on import against a witness.** A generation counter is the wrong
  currency; a *content digest of what the entry depended on* is the right one. If a
  `Thread` could produce a witness (`ContentId` of the file, the ETag, the store's
  revision), an imported entry could be admitted iff every witness still matches
  locally. This is a real change to the thread model, not a cache feature, and it
  belongs to whichever arc takes it.
- **Import the exporter's cut history.** Rejected. A history of cuts on another machine
  says nothing about what has changed on this one; it would turn a meaningless number
  into a meaningless number with provenance.

### 2. The capability fingerprint is in the key, and it is not an identity

`capability_key` is BLAKE3 over the capability's sorted scope strings. It is stable
across processes — the same scope set fingerprints identically anywhere — and that is
exactly the trap: a *scope string* is frequently host-relative (`urn:cap:fs:` grants
name paths; see `ikigai-core-PENDING.md` §18 on host-relative thread names). Equal
fingerprints therefore do not mean equal authority.

The rule an import must follow: **derive the key's capability half from the importing
caller's own capability and refuse anything that does not match.** Never adopt the
exported fingerprint as evidence of authority. An entry computed under root on the
build host must not become readable under a narrow capability here just because the
bundle says so.

### 3. ★ The key does not identify the code — this is the deepest one

`RequestId` hashes the verb, the target IRI and the arguments, and nothing else. It is
deterministic and process-independent (`"ikigai.request.v0"` is even versioned into the
hash), which reads like good news for export. It is not, on its own:

> The resolved-endpoint / evaluation-scope dimension is layered on by the kernel at
> resolution time. — `Request::id`

Inside one process that layering is implicit, because the binding does not move.
Across processes it is the normal case for it to move: the same `urn:cms:graph` is
bound to a different module version, a different mount, a different backing store. Two
instances can agree perfectly on the request id and mean two different computations.

So an importable bundle has to carry, per entry, a claim about **what produced it** —
at minimum the resolved endpoint's `Description::id` and the producing crate's version,
checked against the local binding at import. Without that, a warm cache is a channel
for one build's answers to be served by another build's code. That is strictly worse
than a cold start, and it fails silently.

(The same blind spot has a within-process twin: `ikigai-core-PENDING.md` §14 finding 7
— the kernel's self-description is cached `Never` with an empty thread set, so a
rebind leaves the catalog stale forever. One fix would serve both: a thread the kernel
cuts when a binding changes.)

### 4. Policy metadata must be re-based, never trusted

`hits`, `stored_tick`, `last_used_tick` and `cost_millis` exist for the
[`CachePolicy`](../../crates/ikigai-core/src/cache.rs), not for validity. An import
that carried them verbatim would let a bundle declare itself permanently hot and pin
out locally computed entries. On import: ticks come from the importing cache's own
counter, `hits` starts at zero, and `cost_millis` survives only as a hint (it was
measured on someone else's hardware). This is why the entry type keeps the two kinds of
metadata visibly separate.

`Expiry::At` deadlines are the exception that transfers: they are absolute
milliseconds, and mean the same thing on any kernel whose clock is sane.

### 5. A bundle is untrusted input

An ejected cache crossing a machine boundary is a file of answers you did not compute,
keyed by hashes you did not take. It wants integrity (`ikigai-sign` signs an RDF graph
today; a bundle manifest is the same shape) and it wants a producer identity, so an
operator can say *whose* pre-computation this host will accept. Content-addressing
proves internal consistency — that these bytes are what this `ContentId` names — and
nothing about whether the answer was right.

## What would actually ship, when it ships

A minimum viable ejection, in the order the risk sits:

1. **Export** every entry with `Expiry::Never` and an empty thread set, plus a manifest:
   producer identity, `ikigai-core` version, and per entry the resolved endpoint's
   description id and version.
2. **Import** under an explicit capability, admitting an entry only when (a) the
   manifest's endpoint id and version match the local binding, (b) the key's capability
   half re-derives from the importing capability, and (c) the representation's bytes
   hash to the `ContentId` recorded for them.
3. **Re-base** all policy metadata locally.
4. Everything else — threaded entries, `At` deadlines, cross-authority reuse — waits on
   the thread-witness work in §1, and should stay unrepresentable in the format until
   then rather than representable and refused.

## What 0.1.70 did to keep the door open

- `CacheEntry` and `CacheKey` are named, public, and `serde`-derived. No new dependency:
  `serde` was already a dependency of `ikigai-core`.
- Validity metadata (`edges`, the representation's own `expiry`) and policy metadata
  (`hits`, ticks, `cost_millis`) are separate fields, so §4's re-basing is a field-level
  operation rather than a rewrite.
- `cost_millis` is `Option`, so "not measured" stays distinguishable from "free" —
  across a process boundary that difference is the whole signal.
- Nothing exports. There is no format, no version byte, and no import path to misuse.
