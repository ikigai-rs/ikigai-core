# Declaring action consistency

Status: accepted by Brian, 2026-11-24, as proposed; the follow-up items at the end are filed in the
ledger. The tables are proposed per-verb declarations and rollout gates.
They describe the contracts to publish after verification. Existing declarations are
identified where they already carry part of the promise.

An agent deciding whether to repeat an ambiguous call needs replay semantics. A
replicator deciding whether to reorder writes needs a statement about the facts being
merged. A worker competing for an item needs to know where the winner is decided.
These are independent properties. A keyed append can be safe to resend while still
requiring coordination to allocate a number. A correction can require coordination
while deliberately replacing the effective verdict.

## Contract on an action

Use the normalized `ActionSpec` as the authoring unit, including a distinct contract
for each verb on a multi-verb endpoint. Keep the existing `resend` field and add
optional `monotone` and `coordination` declarations. No field means no promise for that
property. A property can be asserted for a named part of the effect only.

| Property | Meaning |
| --- | --- |
| Idempotent effect | Under its stated identity and retention rules, replaying the same logical call adds no further action effects after the first landed. Compare durable state, emitted events and external charges. Replies may distinguish a fresh write from a replay or a gone resource. |
| Monotone facts | For a named fact set, every successful transition preserves earlier facts. Admissible independent calls over that set commute, so permutations yield equivalent facts. Check inclusion and commutation separately. |
| Coordination point | For a named decision and conflict domain, outcomes and the resulting state match a legal sequential history that respects real-time precedence. Say which authority serializes the decision, which writes count as part of it and what may still need recovery. |

A same-key call may return an existing resource for different content or reject a
collision. Document that policy separately from the replay guarantee. For automatic
resend, require the same effective principal, target, action, normalized arguments
and stable identity within the identity's stated lifetime. A call may have finished
before its reply was lost, or the retry may overlap the original. A same-state
no-op under a frozen clock establishes only a narrower property. In particular,
renewal, a later successful call after a failed attempt, and an unfenced transition
after the state cycles back can all have new effects.

The proposed first increment uses stable, registered identifiers for fact sets and
decisions plus a small authority vocabulary (`store-transaction`, `layer-host`,
`endpoint-instance`). The identifiers select documented, testable semantics. For
example, `claim-epoch` names a ledger item's claim decision and `tuple-attempt`
names a layer subject's pending-to-taken transition. Store transaction means the
same authoritative store. Layer host means a single mounted layer authority.
Endpoint instance means one live endpoint and the locks it shares with explicitly
named peers. None of those scopes implies distributed coordination across
independent authorities. Where one verb has modes with different behavior, publish
only a common machine-readable guarantee initially. Keep narrower modes in prose
until typed predicates on normalized arguments have a consumer and tests.

An append-only log can assign a sequence or time that depends on arrival order.
Treat that as a coordinated write. It can still have a monotone projection onto
immutable identities and payloads, provided those facts survive and independent
writes commute. Use fact set inclusion rather than row counts, so replacing one fact
with another fails the check. Exclude generated names or sequence numbers only
when the contract explicitly defines a projection that omits them. Exposing a
changing selection such as `latest`, an open-work list, or an effective verdict adds
no monotonicity promise for that view. An audit event recording a removal can be
additive while the affected state shrinks. A kernel thread cut similarly changes
which cached answers are fresh. The first declaration pass covers the three
modules below.

[Core action descriptions](../../crates/ikigai-core/src/describe.rs) already carry
`Resend::Safe` and `Resend::Key`. Start by keeping their JSON and RDF encodings.
A named key is an existing declared input. Some useful identities are intrinsic
to validated content, such as a tuple's RDF subject. Record the identity in the
module's contract and use unconditional resend only when every accepted call
has it and the effect guarantee holds. Keep narrower replay properties out of
automatic resend until the action can express and honor the qualifying condition.

## Proposed declarations from the audit

These are the first-pass bindings for the three modules. Write actions without
a checked promise remain unspecified. Reads get an independent read-only
review before MCP emits a read-only hint. The absence of a new declaration
on a read carries no claim about its response or retry behavior.

### Ledger

| Action ID and verb | Resend declaration | Monotone facts | Coordination decision |
| --- | --- | --- | --- |
| `ledger-append` Sink | Existing `Key("key")`. Identity is the key in the ledger, including its tombstone. The unkeyed form allocates again. | Omit. Number and history are order-sensitive. | Store transaction per ledger for key uniqueness and item numbering. |
| `ledger-comment`, `ledger-note` Sink | Existing `Key("key")` for the ledger's comment key space. Exercise note replay with its claim epoch and exercise deletion separately before strengthening the documented lifetime. | Omit. History and comment ordering are observable. | Store transaction for the key and item guards. For note, include the claim epoch fence. |
| `ledger-history` Sink | Existing `Safe` for state effects. A source ID held by a live imported event suppresses a duplicate. Item deletion frees its source IDs. | Additive for distinct source event IDs on live items. Colliding payloads and exposed commit order are outside that projection. Defer publishing until reorder checks cover imported attribution and timestamps. | Store transaction for source ID uniqueness and target liveness. Gate any promise of accurate per-call counts on the race reproduction below. |
| `ledger-claim` Sink and Delete | Omit. Same-holder claim can renew a lease, and an unfenced release can act on a later claim. | Omit. The holder and lease are current state. | Store transaction per item for holder and epoch, with the supplied fence where applicable. |
| `ledger-close`, `ledger-reopen` Sink | Omit. Closing again can update evidence or verdict. Reopen can apply again after an intervening close. | Omit. Open status and the ready set can change in either direction. | Store transaction per item for the transition, claim release on close and status-event numbering. Test evidence separately from transition count. |
| `ledger-item-state` Sink, `ledger-lapse` Sink | Omit. A matching state or lease condition alone does not identify a replay. | Omit. | Candidate store transaction per item for the guarded transition. Test changes between guard read and update. |
| `ledger-item` Sink and Delete, `ledger-purge` Delete, `ledger-label`, `ledger-link`, `ledger-item-about`, `ledger-defer` Sink and Delete | Omit pending effect and fence fixtures. | Omit at whole-action scope. These edit, remove or move visible facts, or add facts with observable history and update state. | Check the specific guards before advertising a decision, including edge and mirror updates for links and the live-to-deleted transition. |
| `alias-scheme` Sink, `alias` Sink and Delete | Omit pending fixtures. | Omit. Alias removal is deliberate. | Candidate uniqueness decision for scheme and value on alias registration, subject to the underlying authority actually enforcing it across ledgers. |
| `ledger-item-sessions`, `ledger-sessions`, `ledger-backfill-closed-at` Sink | Omit initially. A summary replay on the same hand-back is a scoped no-op, while the target is selected from the latest hand-back. A repeated migration or backfill can be checked against its documented scope. | Omit at whole-action scope. Moving summaries removes facts from the old graph. | Verify guards on hand-back association and backfill against changes to current item state. |

Ledger control coverage includes concurrent keyed append, claim compare-and-set,
racing close and reopen, and graph equality after a keyed replay. The same-holder
claim and move replay tests provide counterexamples to unconditional resend.
[Endpoint source](https://github.com/ikigai-rs/ikigai-ledger/blob/f96a9778109911a9fcbccd934ad7d76efcbb457c/src/endpoints.rs)
and [resend tests](https://github.com/ikigai-rs/ikigai-ledger/blob/f96a9778109911a9fcbccd934ad7d76efcbb457c/tests/resend.rs).

### Layers

| Action ID and verb | Resend declaration | Monotone facts | Coordination decision |
| --- | --- | --- | --- |
| `layer-{layer}-say` Sink | Omit. A repeat allocates an entry. A lost reply can leave an entry and a feed event owed. | Omit for ordered channel state. A superseding entry adds an immutable entry and changes `latest`. | Layer host per layer for feed sequence allocation. Treat payload and feed recovery as separate from the allocation decision. |
| `layer-{layer}-tuples` Sink (out) | Proposed `Safe` for action effects, after checking the maintenance boundary. Same validated subject, payload and dropper yields one tuple within retention. After receipt expiry the old subject stays used and replay can conflict. | Omit for pending tuples and ordered feed. | Layer host per layer for subject uniqueness and numbering. |
| `layer-{layer}-tuples` Delete (take) | Omit. Recover a lost result from the receipt when present and visible. A leased tuple can return and be taken again. | Omit for pending tuples. | Layer host for one winner per pending attempt, including take versus revocation. A later lease return starts another attempt. Feed and receipt fault tests bound the recovery promise. |
| `layer-{layer}-receipt` Sink (leased configuration only) | Omit for the whole verb. Same-result `settle=done` is replayable while its receipt survives. `settle=renew` resets expiry from the current clock. | Omit for receipt lease state. | Layer host per receipt attempt for a final result and the race against lapse or renewal. |
| Layer definition, feed, entry, tuple and receipt Source | Omit whole-call read-only hint on routes with inline lease or retention maintenance. | Omit for views over changing state. | Any read-triggered maintenance uses the layer authority. Its existence must be visible to callers making effect-sensitive decisions. |

Test duplicate drops before and after take and across restart, one winner from
competing takes, take versus revocation, a lapse creating another attempt, and a
take that committed with a feed write error. For the retained receipt boundary,
the existing retention test checks conflict after expiration and reopen.
[Layer endpoint source](https://github.com/ikigai-rs/ikigai-layers/blob/003690a6a80d98457d92520709af477e464867dd/src/endpoints.rs)
and [tuplespace tests](https://github.com/ikigai-rs/ikigai-layers/blob/003690a6a80d98457d92520709af477e464867dd/tests/tuplespace.rs).

### Judge

| Action ID and verb | Resend declaration | Monotone facts | Coordination decision |
| --- | --- | --- | --- |
| `judge-{question}-judge` Sink | Omit unconditional resend. Reuse of a recorded current result is a conditional no-op. A failed attempt can spend again after its retry time or a curator clear. | Omit for effective verdict, reservation state and spent records. | Endpoint instance per question, classifier and item digest for admission to a paid attempt, shared with clear for that reservation. Separate endpoint-instance decision per question, source and UTC day for budget admission. The counter and reservation are two writes, and each has its own failure boundary. |
| `judge-{question}-reservation` Delete | Omit pending attempt-fenced replay. A reservation resource can refer to a later attempt. | Omit for current reservation state. | Shared endpoint-instance item lock with judge for clearing an attempt. Audit log and updated reservation need a separate partial-failure check. |
| `judge-{question}-correct` Sink | Omit. A repeat can rewrite timestamps and the last-verdict pointer and can invoke training again. | Omit. The person verdict supersedes previous evidence in the effective view. | Endpoint instance per question, corridor and digest for ordering competing corrections. Gate a stronger atomic verdict-plus-training claim on the partial-write reproduction below. |
| Question, prompt, classifier, model, person, spent, class, why, pending, disagreements and reservation Source | Candidate read-only after verifying composed reads. | Omit for views and rankings. | Omit. |

The budget test synchronizes concurrent judgments and checks the available call
count. Correction tests compare the verdict with the trained classification after
competing writes complete. Those tests say little about visibility halfway
through a correction, or independent hosts sharing storage.
[Judge endpoint source](https://github.com/ikigai-rs/ikigai-judge/blob/62ce21b23c1ec29437cb9f922fc99c087b439f4e/src/endpoints.rs)
and [judge tests](https://github.com/ikigai-rs/ikigai-judge/blob/62ce21b23c1ec29437cb9f922fc99c087b439f4e/tests/judge.rs).

## Checking claims through the kernel

Use a fresh module host, scratch state, a controllable clock and the same
capabilities as real clients. Bind fake downstream endpoints when their external
effects need counting. A fixture names the action ID, verb, applicable bindings
and feature configuration, normalized requests and state projection. It also
specifies the valid outcomes and reads enough state to distinguish a lost update
from a duplicate or a false success. Snapshot the state before and after through
an observer that does not itself trigger the maintenance under test.

1. **Replay.** Compare one call against execution followed by a replay. Drop the
   first response after commit as well as interrupting before commit. Try overlap
   and reopen over the same state. Vary clock, caller, key collisions, key absence
   and relevant resource changes according to the contract. Assert both semantic
   response classes and effect equality, including event count, remaining budget
   and cache freshness where a no-op promises no write.
2. **Monotonicity.** Compare fact sets at each prefix. On fresh copies, run
   admissible independent inputs in both orders, then permute small batches.
   Normalize RDF as a graph or dataset where appropriate. Compare input-to-fact
   associations, not just a checksum of counts. Exercise conflicts as a separate
   check with a specified collision policy.
3. **Coordination.** Use `Race::fresh` and `Race::together` or `Race::copies`
   for simultaneous callers. Use `Interleave` for a specific read/commit gap
   and store or downstream faults. Include conflicting verbs such as close and
   reopen, take and revoke, judgment and clear. For small batches enumerate legal
   sequential histories, preserving real-time precedence, and compare replies and
   durable state against them. For larger batches check bounds and invariants such
   as unique numbers, one winning epoch or attempt, and no budget overrun within
   the documented authority. Include an intermediate observer and a failed
   second write for any claimed all-or-none outcome.

Add a consistency check to the suite alongside failure probes, requiring a
witness for every new assertion in the opted-in module. Missing witness is a
coverage failure with the action and property named. Report the seed, clock,
schedule, response classes and differing facts when a witness fails. Keep
bounded schedules in routine CI and run threaded or repeated stress checks in
a separate job as needed. Passing sampled schedules establishes the tested
cases and scopes. [Conformance suite](https://github.com/ikigai-rs/ikigai-conformance/blob/f00b1f038df44cb63bf6e4332187c467d7037d98/src/suite.rs)
and [race harness](https://github.com/ikigai-rs/ikigai-conformance/blob/f00b1f038df44cb63bf6e4332187c467d7037d98/src/race.rs).

## Manifold and MCP

Use the same metadata in the normalized JSON action contract, RDF Meta face
and `ActionMatch` rows exposed by `urn:kernel:actions`. Resend already makes
that trip. Add optional RDF fields for fact set, decision and authority, and
update the JSON-LD context, SHACL shapes and projection vectors together.
Include declared properties in the content-addressed contract identity.
Omit absent properties from serialization so legacy action digests hold.
Check the effect on MCP names when tool grouping disambiguates contracts by
digest. Publish extensions through the existing action selection and its
capability filters, with no additional disclosure of live locks or holders.
[Selection](../../crates/ikigai-core/src/select.rs) and
[Turtle renderer](../../crates/ikigai-vocab/src/lib.rs).

In MCP `tools/list`, project an independently reviewed read-only action to
`annotations.readOnlyHint`. Use `annotations.idempotentHint: true` for an
unconditional write effect guarantee only. Leave the hint absent for optional
keys and mode-dependent effects. Carry the full contract under a namespaced
`_meta` key such as `ikigai-rs.dev/consistency`, with a short readable tool
description for clients that ignore extensions. A keyed write's description
can say “Resend with the same nonblank key and arguments.” Keep annotations
advisory, require a trusted server, and leave scheduling and replay decisions
to clients. Coordination and monotonicity alone imply neither resend nor a
destructive hint. The projection should assert the same effective contract for
every route grouped into a tool and preserve distinctions across configurations.
[MCP tool annotations and metadata](https://modelcontextprotocol.io/specification/2025-11-25/schema#toolannotations)
and [MCP bridge](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-mcp/src/server.rs#L374).

## Reproductions and candidate follow-ups

The probes below ran against the pinned module sources above using Cargo 1.99.1.
Each can be repeated in its module with the indicated fixture and assertion.
They use a local store, a scratch layer host or a fake model. The normal
ledger `claim_cas`, `close_race`, `keyed_append` and `resend` suites, layers `channel`, `lease`, `retention`, `seq_fault` and `tuplespace`
suites, and judge `judge` and `retry` suites passed with the negative probes
excluded.

### Ledger history can overreport a racing import

The existing `concurrent_re_sends_record_each_event_once` test schedules a
second import of the same source event after the outer call reads the keys and
before its guarded update. In that test, capture the return value of the outer
`send(...).unwrap()` as `answer`. After the existing assertion that one event
is stored, add the following check.

```rust
assert!(answer.contains("recorded 0, already recorded 1"), "{answer}");
```

Run `cargo test --test imported_history concurrent_re_sends_record_each_event_once`.
The stored event count is one, but the outer call reports “recorded 1, already
recorded 0” after the inner call already inserted the event. The response counts
the pre-update batch instead of the committed delta. This preserves the replay
effect and violates a contract that promises accurate per-call outcomes.
[Scheduled import test](https://github.com/ikigai-rs/ikigai-ledger/blob/f96a9778109911a9fcbccd934ad7d76efcbb457c/tests/imported_history.rs#L415)
and [import endpoint](https://github.com/ikigai-rs/ikigai-ledger/blob/f96a9778109911a9fcbccd934ad7d76efcbb457c/src/endpoints/import.rs#L530).

### A layer definition read can write a lease return

Add this probe to `tests/lease.rs` and run
`cargo test --test lease definition_source_does_not_write_the_feed`.

```rust
#[test]
fn definition_source_does_not_write_the_feed() {
    let (mut f, clock) = leased();
    out(&f.kernel, &gonk(), &offer("a")).unwrap();
    take_leased(&f, &bobby_gonk(), &offer_iri("a"), "10m").unwrap();
    let log = f.state.path().join("tasks-bobby/feed.log");
    let before = std::fs::read(&log).unwrap();
    at(&mut f, &clock, 10);
    source(&f.kernel, "urn:layer:tasks-bobby", &[], &tasker(&bobby_gonk()));
    let after = std::fs::read(&log).unwrap();
    assert_eq!(after.len(), before.len(), "definition appended a feed event");
}
```

The feed grows when the definition is read at the expiry time. Reading the
file isolates the effect from a second endpoint call that could trigger the
same sweep. Lease return on access is intentional. The mismatch is with a
blanket Source-as-read-only or unconditional resend guarantee. Establish
whether to expose maintenance effects or move them to an independently
scheduled sweep before adding hints. Retention has a similar boundary, with
existing coverage for expiry triggered by a receipt listing.
[Lease fixtures](https://github.com/ikigai-rs/ikigai-layers/blob/003690a6a80d98457d92520709af477e464867dd/tests/lease.rs)
and [pre-dispatch maintenance](https://github.com/ikigai-rs/ikigai-layers/blob/003690a6a80d98457d92520709af477e464867dd/src/endpoints.rs#L2326).

### A failed shared correction can change the effective verdict

In `tests/judge.rs` add this probe and run
`cargo test --test judge correction_refused_by_training_leaves_the_verdict_unchanged`.

```rust
#[test]
fn correction_refused_by_training_leaves_the_verdict_unchanged() {
    let h = Host::new();
    let before = h.class(HAMMY, &[]);
    let corridor = ikigai_judge::digest_hex(PRODUCT.as_bytes());
    let cap = Capability::scoped([
        format!("urn:cap:judge:read:{Q}"),
        format!("urn:cap:judge:correct:{Q}"),
        h.fs("read", ""),
        h.fs("write", &format!("/person/{corridor}")),
        format!("urn:cap:bayes:read:{Q}"),
    ])
    .with_principal("urn:example:curator")
    .unwrap();
    let outcome = h.issue_as(
        &cap,
        Verb::Sink,
        &format!("urn:iki:judge:correct:{Q}"),
        &[("content", HAMMY), ("label", "spam")],
    );
    assert!(matches!(outcome, Err(Error::Denied(_))), "{outcome:?}");
    assert_eq!(h.class(HAMMY, &[]), before);
}
```

Training is denied and the assertion fails with `spam person-shared` instead
of `pending`. The per-item correction lock orders writers through training.
The person atom has already landed when training fails. An atomic correction
contract needs either a single commit or a documented intermediate state and
recovery path with matching replies.
[Correction implementation](https://github.com/ikigai-rs/ikigai-judge/blob/62ce21b23c1ec29437cb9f922fc99c087b439f4e/src/endpoints.rs#L1888).

### A failed reservation can use budget without a model call

In the existing `a_failed_item_reservation_never_calls_the_model` fixture,
insert the following assertion after verifying zero fake model calls and before
retrying with the regular worker.

```rust
assert_eq!(
    h.text(
        Verb::Source,
        &format!("urn:iki:judge:spent:{Q}:form:2026-10-31"),
        &[],
    ),
    "spent 0 of 100\n"
);
```

Run `cargo test --test judge a_failed_item_reservation_never_calls_the_model`.
It reports `spent 1 of 100`. The spent write precedes the item reservation
write. If the budget counts calls or successfully saved reservations, a failed
second write violates that accounting rule. If the unit is a conservative
admission debit, document the charge before the item reservation and its
recovery policy explicitly. A refusal from the model is a separate case
because it may have incurred cost.
[Reservation fixture](https://github.com/ikigai-rs/ikigai-judge/blob/62ce21b23c1ec29437cb9f922fc99c087b439f4e/tests/judge.rs#L558)
and [budget and reservation writes](https://github.com/ikigai-rs/ikigai-judge/blob/62ce21b23c1ec29437cb9f922fc99c087b439f4e/src/endpoints.rs#L1636).

### Projection and ordering controls

- The existing core test `the_action_manifold_says_whether_a_resend_is_safe`
  asserts keyed and unconditional metadata in Turtle. To reproduce the
  downstream projection gap, pass the same keyed `ActionSpec` to MCP's
  `action_to_tool` and inspect its keys. Source inspection shows it builds
  only `name`, `description` and `inputSchema`, with no structured resend
  policy. The bridge's list path consumes that projection. This is a missing
  consumer for an existing declaration. [Manifold test](../../crates/ikigai-core/tests/resend.rs)
  and [MCP projection](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-mcp/src/lib.rs#L864).
- In ledger `tests/imported_history.rs`, use `seeded()` for each of two
  fresh kernels. Call `send` twice as `importer()` on item `1`, with two Turtle
  edit events dated `2020-02-01T00:00:00Z`. Use source IDs `urn:src:a` and
  `urn:src:b` with edit kinds `defer` and `resume` respectively. Reverse the
  send order on the second kernel. Select `eventKey` and `editKind` from
  the live ledger graph and order by key. Both directions produced the same
  two key-kind pairs. This checks one narrow source fact projection and
  leaves committed timestamps, liveness changes and key collisions for
  the broader fixture.
- In `tests/channel.rs`, create two fresh `fixture()` hosts. Use
  `say(&f.kernel, "talk", &bobby(), &message(body))` with bodies `a` then
  `b` on one host and the reverse on the other. Strip `urn:layer:talk:entry:`
  from each returned IRI and parse the position with `id::seq_of`. The
  `(a, b)` positions are `(1, 2)` versus `(2, 1)`. The same writes admit
  both payloads while assigning different positions. This is deliberate
  coordination over ordered channel state.
  [Channel fixtures](https://github.com/ikigai-rs/ikigai-layers/blob/003690a6a80d98457d92520709af477e464867dd/tests/channel.rs).

## Proposed follow-up items

- **Add scoped consistency fields to action descriptions.** Define fact-set
  and coordination identifiers, preserve undeclared contract digests, and
  round-trip the optional properties through JSON, Turtle and action matches.
- **Check consistency declarations in module conformance.** Add witness
  coverage and regression fixtures for replay, reorder and races, then publish
  the first verified ledger, layers and judge declarations.
- **Project consistency into MCP tools.** Share the capability-filtered
  contract, add conservative annotations, and test optional keys, grouped
  templates and read-triggered maintenance.
- **Report the committed result of a racing history import.** Make recorded
  and existing counts reflect the guarded transaction, then extend the
  deterministic interleave test.
- **Expose maintenance effects on layer reads.** Decide which layer routes
  can claim read-only behavior and test lease return and retention without a
  second request triggering the effect.
- **Define a shared correction's failure boundary.** Preflight prerequisites
  where possible and define how readers and retries observe a failed write
  between the verdict and training.
- **Define judgment budget accounting on reservation failure.** Make budget
  admission and item reservation one recoverable decision or specify which
  incomplete admissions consume the daily limit.
