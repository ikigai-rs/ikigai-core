# Idempotency keys for `Sink`: retrying a write that timed out

**Status:** research note (ledger #2770). Recommends a design and the first three endpoints
to adopt it. Changes no code in the crates; the reproduction lives beside this note in
[`sink-idempotency/repro/`](sink-idempotency/repro/), outside the workspace.

**The question.** A client sends a `Sink`, the transport times out, and the client cannot
tell whether the write landed. If it sends the write again it may write twice: two items, a
comment posted twice, a permanent second entry in an append-only layer. What would let a
client retry safely, and what does each published module do today?

---

## 1. What holds and what does not

The item that asked for this study says no idempotency key exists anywhere and that
`ikigai-throttle`'s `Retry` never re-sends a `Sink`. **Both statements are out of date.**
Since ledger #3381:

- core has `ActionSpec::resend` (`Resend::Safe` and `Resend::Key(name)`,
  `crates/ikigai-core/src/describe.rs`), rendered as `ik:resendSafe` / `ik:resendKey`;
- `ikigai-ledger` 0.24.0 has a `key=` on `append`, `comment` and `note`, and declares
  `resend_safe_with_key("key")` on all three;
- `ikigai-throttle` 0.6.0's `Retry` re-sends a `Sink` when its contract declares a resend
  safe and the call carries the key (`resend_safe`, `src/lib.rs:802`).

**The exposure still holds** wherever a call carries no key, and on every endpoint that has
no key to carry. The existing keys also leave four gaps: a repeat does not answer the first
answer, a reused key is not compared against the content it comes with, keys are not scoped
per principal, and a compare-and-set has no key at all. All of this is reproduced below.

## 2. Reproduction

[`sink-idempotency/repro/src/main.rs`](sink-idempotency/repro/src/main.rs) builds the
published `ikigai-ledger` 0.24.0 and `ikigai-layers` 0.10.4, which depend on
`ikigai-core` 0.3, over an in-memory store. `LossyLink` is the fake transport. It delivers
the request to a real kernel, so the write happens, and then it drops the answer and returns
`Error::Timeout`, as a door, an IPC socket or a stream does when the connection drops
between the write and the reply. The link keeps the answer it dropped so it can be compared
with the retry's answer.

Two callers are tested. `agent_send` is an agent's default behavior: on any transient
failure, send the same request again. `contract_send` follows `ikigai-throttle`'s rule and
sends again only when the contract declares the resend safe for this call. Every finding is
an assertion, so the program fails as soon as a module changes the behavior it reports.

```text
cargo run --manifest-path docs/research/sink-idempotency/repro/Cargo.toml
```

Output from 2026-11-21 (the IRIs vary between runs; everything else is fixed):

```text
- ledger append, no key, agent retry
    lost answer: #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfj
    retry answer: #2 urn:iki:ledger:default:item:01m7q5da1r9y46gdhs2esfk
    items now: 2 (one intended)
- ledger append, no key, contract retry
    not re-sent (resend undeclared without key=); caller sees: timeout: the link dropped after the request was delivered; items now: 1, which the caller cannot learn
- ledger append key=, contract retry
    items now: 1 (no duplicate)
    first answer:  #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfn
    retry answer:  #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfn existing open
    not the first answer byte for byte
- ledger append, same key, different content
    answer: #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfn existing open
    item #1 keeps its first title; no fingerprint is compared
- ledger append, same key after close
    answer: #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfn existing closed (the key is taken forever)
- ledger append, same key, two principals
    writer a: #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfw
    writer b: #1 urn:iki:ledger:default:item:01m7q5crf89y46gdhs2esfw existing open
    b's item is never filed and b is handed a's item
- ledger comment, no key, agent retry
    the comment appears 2 times on #1
- ledger comment key=, contract retry
    one comment (no duplicate)
    first answer:  commented on #1 (urn:iki:ledger:default:comment:01m7q5fde09y46gdhs2esg1)
    retry answer:  commented on #1 (urn:iki:ledger:default:comment:01m7q5fde09y46gdhs2esg1) existing
- ledger item state from=filed to=queued, agent retry
    lost answer: #1 queued (was filed)
    retry answer: conflict: #1 is in `queued`, not `filed`. A transition is a compare-and-set: it names the state it leaves, and another writer moved this item first. Re-read `urn:iki:ledger:item:1:state` and transition from there
    state now: queued (the first attempt succeeded)
- layers say, agent retry
    first entry:  urn:layer:talk:entry:01m7q5cseg0000000000001
    second entry: urn:layer:talk:entry:01m7q5cxbg0000000000002
    the body appears 2 times in the layer, and a layer is append-only; say takes no key and declares no resend

10 findings reproduced.
```

The "two principals" line is the test that the key is shared across principals (both
writers hold grants on one ledger, so this is not a read leak). It shows that two writers'
keys collide: one writer's item silently stands in for the other's.

## 3. Inventory: the non-idempotent writes in the published modules

Read from the latest published sources (`ikigai-judge` and `ikigai-gonk` from their default
branches). "Repeat" means the same call sent again after the first one succeeded. Only rows
in §2 were run; the rest were read from source.

| Module | Write | Repeat today | Key today |
|---|---|---|---|
| ledger | `append` | a second item | `key=`: a natural key, per ledger, taken forever (survives delete and purge); repeat answers `existing <status>`; content not compared |
| ledger | `comment`, `note` | a second comment / note | `key=`, one namespace for both, per ledger; dropped when its item is deleted; repeat answers `… existing`; the item named is not compared |
| ledger | `item:{id}:state` (`from` → `to`) | `Conflict` for work that succeeded | none; the contract says so: "`from` is a state precondition, not a request ID" |
| ledger | `close`, `reopen`, `defer`, `claim`, `claim` Delete with `content=` | the state is idempotent, but `content=` is posted again as an unkeyed comment | none |
| ledger | `item:{id}` edit, `link`, `label`, `about` | idempotent state, plus one more edit event per call | none |
| ledger | `history` (import) | `recorded 0, already recorded N` | per-event source id; declared `resend_safe()` |
| layers | `say` (and `join` lobby) | a permanent second entry | **none** |
| layers | tuplespace `out` | same answer, no new event | the caller-minted subject: per dropper, compares the content digest, kept for the layer's life. **This is already the design recommended here, apart from the window.** |
| layers | tuplespace `take` (Delete) | `Conflict`, even for the same taker | none |
| store | `update`, `graph-update` | whatever the SPARQL does (a counter increments again); always answers `updated` | none |
| store | `load` | set semantics, but blank nodes are renamed, so a document that has them adds them again (read from source, not run) | none |
| judge | `judge:{q}` | `unchanged …` once decided; if the first reply was lost before an outcome was recorded, `Conflict` until a curator clears it | content digest per question |
| bayes | `{m}:train` | `unchanged …: already …` | content digest |
| script | `{name}` publish | same version, one more `publish` event; with `if-version=` a retry is `Conflict` | `if-version` (compare-and-set, not a key) |
| script | `{name}:runs`, `eval` | **runs the effects again**, under a new run id | none |
| intray | `urn:space:{n}` drop | the same id while the tuple is pending; **fires again** once it has been settled | content address, pending tuples only |
| nl | `nl-sparql` drafting | calls the model again; a different output is a new draft, `save=` overwrites | none |
| gonk | `layers:say:{l}`, `layers:ask` | become layer `say`, so they duplicate | none |
| gonk | ledger forms (`act`) | the ledger target's behavior; gonk's own forms send no `key` | passes `key` through if given |

**Callers that send writes again:**

| Caller | Sends again | Reads the contract? |
|---|---|---|
| `ikigai-throttle` `Retry` | idempotent verbs; a `Sink` only when declared and keyed | yes |
| `ikigai-throttle` `Failover` | `is_idempotent()` verbs only: never a declared-safe `Sink` | no |
| `ikigai-embedded` `connect.retry` (CLI `--connect`, remote mounts) | through throttle `Retry`; whether `resend` survives the remote Meta encoding is unverified | through throttle |
| `ikigai-ipc` redial | any verb if the frame was never fully sent; reads only after a read-phase break, so a `Sink` whose reply was lost surfaces as a transient error | no (verb and phase) |
| `ikigai-layers` gateway `ask` | re-drops under a NEW correlation id on "already names" | no |
| agents (MCP tool calls, scripted loops) | whatever they choose: by default, everything that failed transiently | MCP carries no idempotency hint |

**Where a write can land after its caller was told it timed out:** throttle `Timeout` and
`Retry`'s overall limit (the inner future is dropped, and the error is transient),
`ikigai-ipc`'s per-call deadline (the server keeps running), the layers gateway's `ask_as`
deadline, and an outer timeout around the store (the store's own budget settles its commit
under a lock, but an outer future being dropped is not that budget). Core owns no timer and
abandons nothing.

**No transport carries a request id.** `ikigai-wire`'s calls have no idempotency or request
field, and IPC matches replies by position. A key must therefore be something every
transport already carries: an argument.

**Delete is not as idempotent as `Verb::is_idempotent` says.** Core treats `Delete` as
idempotent (RFC 9110), and throttle re-sends it freely. Two published Deletes do different
work on a repeat: an intray take with no selector (a FIFO pop) or with `match=` takes a
**different** tuple, and a layers tuplespace `take` answers `Conflict` to its own retry.
Several others (ledger item delete, `label` Delete, bayes `untrain`) answer `NotFound` on a
repeat, which reports a success as a failure. This was read from source, not run, and it is
outside this note's question, so it is listed as a follow-up.

The intray case has since been run (ledger #4063).
[`sink-idempotency/repro-delete/`](sink-idempotency/repro-delete/) builds the published
`ikigai-intray` 0.7.1 behind `ikigai-throttle` 0.6.2's `Retry`, over a fault that lets the
take land and then loses its answer:

```text
cargo run --manifest-path docs/research/sink-idempotency/repro-delete/Cargo.toml
```

Output from 2026-11-24 (which tuple the first take pops may vary between runs; everything else is fixed):

```text
- intray take, its contract
    Verb::Delete.is_idempotent() = true; the take declares resend: None
    so throttle Retry treats every take as safe to send again
- intray take with no selector, behind throttle Retry
    attempts: 2
    lost answer:  job 1
    retry answer: job 0
    tuples pending now: 0 (one take was intended, which would leave one)
    the lost tuple is gone from the space and was handed to nobody
- intray take match=, behind throttle Retry
    attempts: 2
    lost answer:  <urn:a> a <urn:Person> .
    retry answer: <urn:b> a <urn:Person> .
    tuples pending now: 0 (one take intended)
- intray take tuple=<id>, behind throttle Retry
    attempts: 2
    lost answer:  only
    retry answer: not found: no tuple `3bf2bad95f75f3f8f212d5a59d92423554fe45cbcbad5e9217dccb218bc7fc29` to take in space `q`
    the take succeeded and the caller is told it found nothing

4 findings reproduced.
```

Core now lets such a Delete say so: `ActionSpec::resend_unsafe()` (`Resend::Unsafe`,
`ik:resendSafe false`), read through `Description::may_resend(verb, given)`, which answers
the declaration when there is one and `Verb::is_idempotent` when there is not. When intray
declares it on its take and throttle's `Retry` asks `may_resend` instead of the verb, the
no-selector case flips from "two tuples taken" to "one attempt, the caller sees the
timeout, and the second tuple is still there".

## 4. Prior art

- **IETF `Idempotency-Key`** ([draft-ietf-httpapi-idempotency-key-header-09](https://datatracker.ietf.org/doc/draft-ietf-httpapi-idempotency-key-header/),
  still a draft). The key is an opaque string, and a UUID is recommended. A resource MAY store
  a fingerprint of the payload with the key. A reused key with a different payload SHOULD get
  422, and a key whose first request is still in flight SHOULD get 409. Expiry is the
  resource's policy and SHOULD be documented. After completion, a repeat gets a response
  "consistent with the request being completed", not necessarily the first response.
- **Stripe** ([idempotent requests](https://docs.stripe.com/api/idempotent_requests)). Keys
  are kept for at least 24 hours. Stripe saves the status code and body of the first request
  and replays them, "including `500` errors". A reused key with different parameters is an
  error. Validation failures and concurrency conflicts are not saved, so the caller may
  retry them.
- **AWS** ([Making retries safe with idempotent APIs](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/),
  EC2's `ClientToken`). The token is supplied by the caller rather than derived from the
  parameters, because two identical requests may really be two intended actions. It is keyed
  by the customer and the token. The record and the change are one ACID operation. A token
  reused with different parameters is a validation error. The token is remembered for the
  resource's lifetime plus an interval. A repeat gets a "semantically equivalent" answer
  rather than an `AlreadyExists` error, because that error pushes the work back onto the
  client.
- **In this ecosystem:** the layers tuplespace `out` (a caller-minted subject, per dropper,
  with the content digest compared) and core's content holds (`MemoryContentStore::put_for`,
  which scopes a hold to the principal of the capability).

## 5. Design

### 5.1 Two different things are both called "key"

The ledger's `key=` is a **natural key**: the caller's name for the item, such as an
importer's issue id. It is unique per ledger and is taken forever, deliberately, so that a
replay never brings back an item someone deleted. An **idempotency key** is a **request
token**: it is the caller's, it identifies one attempt at one write, it is remembered for a
window, and a repeat under it gets the first answer. Merging the two is what produced the
gaps in §2:

- the cross-principal collision, because a natural key is shared by everyone writing to the
  ledger;
- the `existing` marker, because a natural key reports what it found rather than replaying;
- the compare-and-set with no key at all, because a state transition has no natural name.

**Recommendation:** keep `key=` as the natural key, with the one fix below, and add a
separate request token for the cases a natural key cannot serve.

### 5.2 The request token

- **Where it lives: a request argument, `idempotency-key=`.** It is not a wire field. Every
  transport already carries arguments, and the wire has no request id to extend, so adding
  one would be a flag day across ipc, quic, wire and every polyglot peer. As an argument, the
  token is declared in the contract with the existing `Resend::Key("idempotency-key")`, so
  throttle's `Retry` honors it with no change. An HTTP door maps the `Idempotency-Key`
  header onto the argument. The name matches the HTTP header, so nobody has to learn two
  names. Its shape is the ledger key's alphabet (ASCII letters, digits, `-._~:`, at most 256
  characters) so that it survives gonk's path mapping.
- **Scope: per principal and per endpoint.** The remembered record is indexed by (endpoint
  resource, `capability.principal()`, token), exactly as a content hold is. Two principals
  may use the same token without seeing each other's answer. A call with no principal shares
  one unnamed scope, the same rule `Holder::Root` follows. Doors always mint a principal, so
  that scope matters only to an embedded host.
- **Fingerprint: the request minus the token.** The record stores `Request::id()` (blake3
  over verb, target and every argument; `ArgRef::Content` contributes its content id)
  computed with the token argument removed. The principal is not part of it, because it is
  already part of the scope. A repeat whose fingerprint differs is refused with
  `InvalidArgument` naming `idempotency-key`, saying it "was used for a different request",
  as IETF's 422 and Stripe's error do. It is never answered as a match.
- **What a repeat answers: the first answer, byte for byte.** That means the recorded
  `Representation`'s media type and bytes, with no `existing` marker. If the stored answer
  read differently from the first, a client could not use it in place of the first, and the
  point of the token is that the client never has to know a retry happened. An endpoint that
  wants to say "this was a replay" can say it in its trace or log, not in the answer.
- **Which outcomes are recorded.** The record holds a success, and a permanent failure the
  endpoint produced after it decided on the write (a `Conflict` from a compare-and-set, for
  example). It does not hold a refusal made before the endpoint ran (`Denied`,
  `InvalidArgument`, a limiter, an unbound target), and it does not hold a transient
  failure. Those wrote nothing, so a retry should run again, as Stripe does.
- **In flight.** A repeat that arrives while the first attempt holds the token is answered
  `RetryAfter`, because it is transient and the throttle already knows how to wait. It is
  not answered `Conflict`, which is the IETF draft's 409 and would read here as permanent.
- **How long: at least 24 hours.** The window is configurable per host and published in the
  contract (a new `ik:resendWindow` beside `ik:resendKey`), as the IETF draft asks. A token
  presented after its window is a new request. That is the stated contract, and it is why a
  natural key stays the right tool for "file this once, ever".
- **Atomic with the write.** The record is written in the same store update as the write it
  guards, as the ledger's `FILTER NOT EXISTS` guard already is, and as AWS requires. That
  makes it an **endpoint's** job, not a kernel overlay's. An overlay holding records in
  memory could not commit with the endpoint's write, and would forget every record on a
  restart, which is the moment retries are most likely.

### 5.3 How it composes

- **With capabilities.** The kernel's capability floor runs before dispatch on every
  attempt, so a replay is admitted only under a capability that may make the write now. A
  revoked grant cannot read a stored answer. Scoping by principal means that a token is
  never a way to learn another principal's answer.
- **With the cache.** A `Sink` is never cached, and the replay record must not live in the
  representation cache. Cache eviction is budget-driven, so it would turn "remembered for 24
  hours" into "remembered until memory pressure". One real cost remains: the kernel cuts
  the target's thread on every dispatched `Sink` (ledger #1105), so a replay that writes
  nothing still invalidates cached reads, which gonk's mirror already works around. A
  follow-up asks whether an endpoint should be able to answer "nothing written" so that the
  cut is skipped. Until then a replay is correct but costs a cache flush.
- **With compare-and-set.** A state transition under a token answers its retry with the
  first answer (`#1 queued (was filed)`). A different caller, or the same caller with a
  different token, still gets the `Conflict`. This keeps "already there means success" from
  hiding a real racer, which is why the token cannot be replaced by treating the target
  state as success.
- **With the natural key.** A call may carry both. The token answers a retry, and the
  natural key answers a re-run (a hook fired twice, an import run again).

### 5.4 The one fix to the natural key

A reused `key=` with different content must not silently answer `existing`. Store a
fingerprint of the filing (the title, the body and the labels it set) with the key. On a
match, answer `existing` as today. On a mismatch, answer `existing` with a `differs` flag
(a fourth word in the plain face, `"differs": true` in JSON) rather than refusing. A refusal
would break every re-sync that legitimately edits a title upstream. The flag lets a fan-out
that reuses fixed keys across waves see that it landed on last wave's closed item.

### 5.5 What core provides

The contract already exists (`Resend::Key`). The core pieces each endpoint should not have
to rewrite are:

- an `IdempotencyKey` parser with the ledger's alphabet;
- `Request::fingerprint_without(arg)`;
- a `Replay` record type (scope, fingerprint, media type, bytes, outcome, recorded at) with
  a stable byte encoding an endpoint stores beside its write;
- the `ik:resendWindow` vocabulary term.

Core stores nothing itself, and no default behavior changes.

## 6. The first three endpoints

1. **ledger `append`**, `idempotency-key=` beside `key=`, plus the `differs` fix. This is
   the most retried write (agents file items) and the duplicate is a visible second number.
2. **ledger `comment`** (and `note`, which shares its key namespace), `idempotency-key=`.
   Every worker posts comments, and the same record, written by `close`, `reopen`, `defer`
   and `claim` for their `content=` note, ends the duplicate comment those verbs post on a
   retry.
3. **layers `say`**, `idempotency-key=`. It has no key today, and an append-only layer
   cannot remove the duplicate later. Its tuplespace `out` already shows the layer can keep
   a per-author record that compares content.

The reproduction's assertions are what each adoption flips. When an endpoint adopts the
token, the matching case in `repro/src/main.rs` changes from "two copies" to "one copy, and
the retry's bytes equal the lost answer's".

## 7. Proposed follow-up items

1. **core: an idempotency-key toolkit for Sink endpoints.** Add `IdempotencyKey` (the ledger
   key alphabet), `Request::fingerprint_without`, a `Replay` record with a stable encoding,
   and `ik:resendWindow` in the vocabulary. Additive, with no stored state in core. Tests pin
   the fingerprint's independence from argument order and the token's exclusion.
2. **ledger: `idempotency-key=` on `append`, scoped per principal, replaying the first
   answer.** Commit the record in the append's own update. A mismatch is `InvalidArgument`,
   and an attempt still in flight is `RetryAfter`. Flip the repro's append case.
3. **ledger: a `differs` flag when a reused `key=` comes with different content.** Store a
   fingerprint of title, body and labels with the natural key, and answer
   `existing <status> differs` (JSON `"differs": true`).
4. **ledger: `idempotency-key=` on `comment` and `note`.** Then extend the token to
   `close`, `reopen`, `defer` and `claim`, whose `content=` note is an unkeyed comment, so
   their retries stop duplicating the note.
5. **layers: `idempotency-key=` on `say`.** Use a per-author record that compares the
   content, as tuplespace `out` does, and declare `resend_safe_with_key`.
6. **ledger: `idempotency-key=` on the state compare-and-set.** A retry gets the first
   transition's answer, and a racer still gets `Conflict`.
7. **throttle: `Failover` should honor declared resend safety.** It advances only for
   `is_idempotent()` verbs, so a keyed `Sink` that `Retry` would re-send is never failed
   over.
8. **core: say which Deletes are not idempotent.** An intray take with no selector takes a
   different tuple on a repeat, and a tuplespace `take` answers its own retry with
   `Conflict`. Yet `Verb::is_idempotent` says every `Delete` is, and throttle re-sends them.
   Let a Delete declare `resend` too, and reproduce the intray case first. *Done in core
   (ledger #4063): `ActionSpec::resend_unsafe` and `Description::may_resend`; intray and
   layers declaring it on their takes, and throttle's `Retry` and `Failover` asking
   `may_resend`, remain.*
9. **core: let an endpoint say a dispatched write wrote nothing.** Then a replay stops
   cutting the target's cached reads.
10. **embedded/resolve: confirm `resend` survives a remote mount's Meta.** If it does not,
    `connect.retry` never re-sends a keyed `Sink` across a mount.
11. **mcp: project resend safety into tool annotations.** That way an agent learns from the
    tool itself which writes it may retry, and under which argument.
