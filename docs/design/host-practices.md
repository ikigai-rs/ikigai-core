# A host's published practices as a resource

Status: proposed. Design note only; no runtime, vocabulary or wire change. Evidence is core
[`b09fcea`](https://github.com/ikigai-rs/ikigai-core/tree/b09fcea6c07568e73bb566cef285db7535d815a0).
Ledger #4512. Source: Brian, 2026-11-27, from a policy paper on accountable agents: a service
publishes what it retains and for how long, how its keys are handled and rotated, and how it
discloses on request, and adheres to that.

## The problem

A host (the `ikigai` binary, gonk) keeps things: ikigai-log segments, sessions, the ledger. It
holds keys: ikigai-secret stores, QUIC certificates. It honors revocation within some bound
(ledger #4507), and it answers, or does not answer, requests for disclosure. None of that is
stated anywhere a client can read, and nothing checks that the host does what a reader would
assume. This note proposes a resource that states each practice, and pairs every statement with
the check that confirms it, the way [ACCESSIBILITY.md](../../ACCESSIBILITY.md) pairs each check
with Pass, Fail or Blocked and its evidence.

## What was checked first

The item is a claim that the gap exists, so it was checked against this revision before
designing anything.

- **No resource or term exists.** No `urn:host:` resource, no kernel operation (the list in
  `crates/ikigai-core/src/kernel_ops.rs`), and no term in `crates/ikigai-vocab/src/vocabulary.ttl`
  describes retention, key handling, revocation or disclosure. A case-insensitive search of
  `crates/ikigai-vocab` for `retention`, `disclos` and `practice` finds one match, a comment
  saying a count "discloses nothing", and no term.
- **The stated stance is not yet true of what the kernel emits.** The item's stance is that
  nothing tracks individuals, only aggregate counts. Today every `TraceEvent` carries the
  capability's scope set (`crates/ikigai-core/src/kernel.rs`, `TraceEvent::capability`), and a
  capability that names a principal carries it as the scope `urn:cap:principal:<iri>`
  ([principal-capability.md](principal-capability.md)). That note also records that
  ikigai-log's `Principal` is built from `capability.principal()` per request and written to a
  `principal=` column, and that `LogTracer` writes one `cap=` column per scope
  ([sub-request-authority.md](sub-request-authority.md)). So a host that logs traces through
  ikigai-log records, per request, which principal asked for what. Publishing "nothing tracks
  individuals" over that log would be a false statement.

The second finding shapes the design: a statement is published only with a check, and the first
per-host task is to run the non-collection check and fix what it finds before the statement goes
out. A host must never publish a practice it would fail.

## Proposed resource

`urn:host:practice`, a Source each host serves, answering `Source` with the current statements
and `Meta` with its ordinary description. The request name is illustrative, subject to vocabulary
review. What it answers is named stably (see "Names" below): the host's practices across versions
are `urn:iki:practice:host:{host}`, and the current version is
`urn:iki:practice:host:{host}:version:{n}`.

Why a resource rather than only the host's `Meta`: `Meta` describes an endpoint's contract (its
verbs, inputs and faces), and a host is many endpoints with no single one to describe. Practices
are facts about the deployment, not a contract of any one endpoint. `urn:kernel:topology` may
link to it with one proposed property, `ik:practice`, whose object is
`urn:iki:practice:host:{host}`, so a reader that finds the host finds its practices.

Faces:

- `text/turtle` and `application/ld+json`: the statements as data, in the vocabulary below.
- `text/plain`: one statement per line, in words, each followed by its check's result, for
  example `Log segments are kept 30 days, then deleted. Check: Pass, retention scan, 2026-11-27.`
  It follows ACCESSIBILITY.md's rules: the result is a word, never a color or a code alone, and
  no terminal escapes or markup.

Authority: readable by anyone who can reach the host, with no capability, because a practice a
client cannot read before trusting the host is no use to it. That makes disclosure a design
constraint (see below): the resource states *kinds* and *bounds*, never paths, key material, key
identifiers, principal IRIs or counts that identify one party.

Caching: the answer changes only when the host's configuration changes or a check reruns, so it
carries a thread (proposed `urn:iki:practice:host:{host}`) that the host cuts on either, and an expiry no
later than the earliest check's due date, so a reader never caches a result past the date it
stops being current.

## A statement and its check

Each statement says one thing and names how it is confirmed.

| Part | Meaning |
|---|---|
| Area | One of: retention, non-collection, keys, revocation, disclosure. |
| Subject | What it is about: a store kind (`ikigai-log segments`, `sessions`, `the ledger`, an ikigai-secret store, the QUIC certificate). |
| Claim | The practice, as data: a duration, a bound, a location kind, a rotation interval, or a never. |
| Source of the value | Where the host reads it: its effective configuration key. A value is generated from configuration, never typed as prose, so the statement cannot drift from what the host runs. |
| Check | Method (test, scan, probe or review), the command or test name, how often it reruns. |
| Result | Pass, Fail or Blocked, with evidence, date and the host's revision. |

Result rules, borrowed from ACCESSIBILITY.md:

- A check that has not run is Blocked, with why. A statement is never served without a result.
- A check that is due and has not rerun is Blocked, with its last date kept.
- A Pass covers only its stated scope; the check says what it does not cover.
- A Fail is served as Fail, not hidden: the resource says what the host does, and a statement
  whose check fails is either fixed or withdrawn, in a new version.

## The statements and how each is checked

| Area | Statement a host makes | How a test or scan confirms it |
|---|---|---|
| Retention | ikigai-log segments are kept for at most *D*, then deleted. | Scan: list segment files and fail if any is older than *D* plus the sweep interval. Test: with a test clock, write a segment, advance past *D*, run the sweep, assert it is gone. |
| Retention | Sessions expire after *D* idle and *M* absolute, and are then removed from storage. | Test: create a session, advance a test clock past each bound, assert the door refuses it and the store holds no record. Scan: count stored sessions older than *M*; must be zero. |
| Retention | Ledger items and comments are kept for *D* (or indefinitely, said as such). | Scan: oldest item's age against *D*; a stated "indefinitely" is checked by stating it, and is reviewed. |
| Non-collection | Nothing records which principal made which request; only aggregate counts are kept. | Test: run a request under a capability naming a principal, then scan every log segment, trace sink and metrics store for that principal IRI; any hit is Fail. Scan, on each release: the log's column set contains no `principal=` and no `cap=urn:cap:principal:` column. Today's finding above says this is Fail until the log is changed. |
| Non-collection | Aggregate counts are not broken down finer than *k* parties. | Test: a count with fewer than *k* contributors is suppressed or merged in every face. |
| Keys | Each ikigai-secret store's kind and protection (for example, an OS keychain, or a file encrypted at rest), never its path or contents. | Probe at startup: the host compares the served kind with the store it actually opened, and refuses to serve the statement on mismatch. Scan: no key material or key identifier in any log segment. |
| Keys | The QUIC certificate rotates at least every *R*; the private key never leaves the host. | Probe: read the served certificate's validity window from a client and fail if its age exceeds *R* or its remaining life is under the renewal margin. Review: the key file's permissions and that no export path exists. |
| Revocation | A revoked capability or session stops being honored within *B* (ledger #4507). | Test: revoke, then repeat a cached read and an uncached read at *B* under a test clock; both must be refused. The bound is whatever ledger #4507 settles; this note only publishes it. |
| Disclosure | How a request for disclosure is made, who decides, what is released, and in what time. | Review: the stated contact and process exist and are reachable. Test: a disclosure request for data the host says it never records returns that it has none, and the non-collection scan above backs that answer. |

Every *D*, *M*, *R*, *k* and *B* is the host's own value, read from its configuration. This note
fixes none of them.

## Names

No blank nodes: every version, statement and check has a stable IRI, so two answers diff, a
reader can cite one statement, and `prov:wasRevisionOf` links named versions. Every partition
is by a named key.

| Resource | IRI |
|---|---|
| A host's practices, across versions | `urn:iki:practice:host:{host}` |
| One version of them (an `ik:PracticeSet`) | `urn:iki:practice:host:{host}:version:{n}` |
| One statement in that version | `urn:iki:practice:host:{host}:version:{n}:statement:id:{id}` |
| That statement's check | `urn:iki:practice:host:{host}:version:{n}:statement:id:{id}:check` |

`{host}` is the host's name, `{n}` a version number that only increases, and `{id}` a short,
stable name for the statement (`log-retention`, `principal-noncollection`) that keeps its value
from version to version, so `…:version:3:statement:id:log-retention` and
`…:version:2:statement:id:log-retention` are the same practice in two versions. A version is
`prov:specializationOf` the host's practices and `prov:wasRevisionOf` the version before it.
A check's IRI is fixed within a version; a rerun changes its result, date and summary in the
answer, and cuts the thread, without minting a new version, because a version changes what the
host promises and a rerun only reports on it.

## Example

```turtle
@prefix ik:   <https://ikigai-rs.dev/ns#> .
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .

<urn:iki:practice:host:example:version:2> a ik:PracticeSet ;
    prov:specializationOf <urn:iki:practice:host:example> ;
    prov:wasRevisionOf <urn:iki:practice:host:example:version:1> ;
    ik:practiceEffective "2026-11-27"^^xsd:date ;
    ik:practiceStatement <urn:iki:practice:host:example:version:2:statement:id:log-retention> ,
        <urn:iki:practice:host:example:version:2:statement:id:principal-noncollection> .

<urn:iki:practice:host:example:version:2:statement:id:log-retention> a ik:PracticeStatement ;
    ik:practiceArea ik:RetentionArea ;
    ik:practiceSubject "ikigai-log segments" ;
    ik:keptFor "P30D"^^xsd:duration ;
    ik:practiceConfigKey "log.retention" ;
    ik:practiceCheck <urn:iki:practice:host:example:version:2:statement:id:log-retention:check> .

<urn:iki:practice:host:example:version:2:statement:id:log-retention:check> a ik:PracticeCheck ;
    ik:checkMethod ik:ScanMethod ;
    ik:checkCommand "segment age scan" ;
    ik:checkInterval "P1D"^^xsd:duration ;
    ik:checkResult ik:PassResult ;
    ik:checkSummary "no segment older than 31 days" ;
    ik:checkDate "2026-11-27"^^xsd:date .

<urn:iki:practice:host:example:version:2:statement:id:principal-noncollection> a ik:PracticeStatement ;
    ik:practiceArea ik:NonCollectionArea ;
    ik:practiceSubject "which principal made which request" ;
    ik:neverRecorded true ;
    ik:practiceCheck <urn:iki:practice:host:example:version:2:statement:id:principal-noncollection:check> .

<urn:iki:practice:host:example:version:2:statement:id:principal-noncollection:check> a ik:PracticeCheck ;
    ik:checkMethod ik:TestMethod ;
    ik:checkResult ik:FailResult ;
    ik:checkSummary "the log writes a principal column per request" .
```

The second statement is what a host would serve if it published today: a Fail, stated as one.
A check whose evidence is itself a resource (a CI run, a published test report) also names it
with `ik:evidence <…>`; the summary stays a sentence, and states a result, never raw scan
output.

## Proposed vocabulary

Proposed only; none is added to `vocabulary.ttl` by this note.

| Term | Kind | Meaning |
|---|---|---|
| `ik:PracticeSet` | class | One version of a host's published practices. |
| `ik:PracticeStatement` | class | One practice, with its check. |
| `ik:PracticeCheck` | class | How a statement is confirmed, and its last result. |
| `ik:practice` | property | From a host or topology to its practices, `urn:iki:practice:host:{host}`. |
| `ik:practiceStatement`, `ik:practiceCheck` | properties | Set version to statement; statement to check. |
| `ik:practiceArea` | property | A concept of the scheme `ik:PracticeArea`: `ik:RetentionArea`, `ik:NonCollectionArea`, `ik:KeyArea`, `ik:RevocationArea`, `ik:DisclosureArea`. |
| `ik:practiceSubject` | property | What the statement is about, as a store kind in words (`xsd:string`). |
| `ik:keptFor`, `ik:neverRecorded`, `ik:rotatesWithin`, `ik:honoredWithin`, `ik:storeKind`, `ik:disclosureProcess` | properties | The claim. Durations are `xsd:duration`. |
| `ik:practiceConfigKey` | property | The configuration key the value is read from. |
| `ik:practiceEffective` | property | The date a version took effect; earlier versions link with `prov:wasRevisionOf`. |
| `ik:checkMethod` | property | A concept of the scheme `ik:CheckMethod`: `ik:TestMethod`, `ik:ScanMethod`, `ik:ProbeMethod`, `ik:ReviewMethod`. |
| `ik:checkResult` | property | A concept of the scheme `ik:CheckResult`: `ik:PassResult`, `ik:FailResult`, `ik:BlockedResult`. |
| `ik:checkSummary` | property | The result's evidence in one sentence (`xsd:string`): what was found, never raw output. |
| `ik:evidence` | property (existing, reused) | A resource that shows the result, by IRI: a CI run, a test report. Optional. |
| `ik:checkCommand`, `ik:checkInterval`, `ik:checkDate`, `ik:checkRevision` | properties | The command or test name, the rerun interval, the date and the host revision the check ran against. |

### Names chosen, and the existing terms they avoid

An earlier draft reused four published terms whose meaning does not fit. Under RDFS, a domain
types every subject that uses the property, so reusing one asserts something false about the
subject.

| Slot | Chosen | Existing term it avoids | Why |
|---|---|---|---|
| What a statement is about | `ik:practiceSubject` | `ik:about` | `ik:about` has `rdfs:domain ik:Explanation`, so every statement would be inferred an `ik:Explanation`, and it ranges over browse resources, not words. |
| A check's result | `ik:checkResult` | `ik:result` | `ik:result` has `rdfs:domain ik:Process` (a plan's answering step), so every check would be inferred an `ik:Process`. |
| A review check's method | `ik:ReviewMethod` | `ik:Review` | `ik:Review` is already a class, an archived LLM review pass; using it as a method value would make a published class a method too. The other methods take the same `…Method` form, so none is a bare word. |
| The result's evidence as words | `ik:checkSummary` | `ik:evidence` with a literal | `ik:evidence` has `rdfs:range rdfs:Resource` and means a resource by IRI. It is reused, with an IRI, where a check's evidence is a resource; the sentence gets its own property. |

The other slots were renamed type first, so none is a bare word another family could want next:
`ik:statement` → `ik:practiceStatement`, `ik:checkedBy` → `ik:practiceCheck`, `ik:area` →
`ik:practiceArea`, `ik:valueFrom` → `ik:practiceConfigKey`, `ik:effective` →
`ik:practiceEffective`, `ik:method` → `ik:checkMethod`, `ik:command` → `ik:checkCommand`,
`ik:every` → `ik:checkInterval`, `ik:checkedOn` → `ik:checkDate`, `ik:revision` →
`ik:checkRevision`. The area, method and result values are concepts of a singular scheme each,
named like `ik:LayerKinds`'s values (`ik:TalkKind`): qualifier, then the scheme's type. A scheme
and its property differ only in case (`ik:CheckResult`, `ik:checkResult`), as `ik:FailureClass`
and `ik:failureClass` do. None of the chosen names is declared in `vocabulary.ttl` today, in any
case.

A shape in `shapes.ttl` would require every statement to have exactly one check with a result,
every Pass or Fail to have a summary and a date, and every version, statement and check to be an
IRI, never a blank node.

## Adherence over time

A practice that changes is a new version, never an edit in place: the host serves the current
`ik:PracticeSet` and keeps earlier ones readable at their own
`urn:iki:practice:host:{host}:version:{n}` IRIs, linked by `prov:wasRevisionOf`, so a reader can
learn what applied on a date. A shortened retention applies to data already held; a lengthened
one applies only to data collected after it takes effect, and the statement says so. The host
regenerates the set at startup from its effective configuration, so a configuration change that
would make a statement untrue changes the statement.

## Disclosure risks of the resource itself

The resource is public, so it must not become a map for an attacker or a tracker:

- Store kinds, not paths. Rotation intervals, not certificate serials or key identifiers.
- No per-party numbers; aggregate counts only above *k*.
- Check evidence states results, not raw scan output, which could hold the very records the
  check looks for.
- A Fail says which practice fails, which is information an attacker could use (for example, a
  revocation bound not met). That is the price of being accountable; a host fixes or withdraws
  the statement rather than hiding the result.

## Task lists

### ikigai-core and ikigai-vocab

- [ ] Review and land the terms above in `vocabulary.ttl`, with a shape in `shapes.ttl`.
- [ ] A small helper that renders a `PracticeSet` to its Turtle, JSON-LD and `text/plain` faces,
      so hosts do not each write the plain-text face; tests for each face, including that
      results are words.
- [ ] Decide the reserved request name (`urn:host:practice`), the `urn:iki:practice:host:{host}`
      family and the `ik:practice` link from `urn:kernel:topology`.

### ikigai-log (a library each host configures)

- [ ] Make the principal and principal-scope columns optional and off by default, or replaced by
      aggregate counts, so a host can pass the non-collection check.
- [ ] Expose the effective segment retention and its sweep interval for the host to publish.
- [ ] A retention test with a test clock, reusable by hosts.

### ikigai-secret (a library each host configures)

- [ ] Report the kind and protection of the store actually opened, for the startup probe.
- [ ] A test that no secret value or identifier reaches a log line.

### The `ikigai` binary

- [ ] Run the non-collection check against its own log configuration and fix what fails, before
      publishing anything.
- [ ] Serve `urn:host:practice` from its effective configuration: log retention, session bounds,
      secret store kind, QUIC certificate rotation, the ledger #4507 revocation bound, and a disclosure
      process.
- [ ] Wire each check: the scans on a schedule, the tests in CI, the certificate probe from a
      client; record results with evidence and date.
- [ ] Cut the `urn:iki:practice:host:{host}` thread on configuration change and on each check
      rerun.

### gonk

- [ ] Run the non-collection check against its log and its ledger: ledger items and comments
      name their author, which is a record of a party by design, so the statement for the ledger
      must say so plainly rather than claim nothing is recorded.
- [ ] State session bounds for its HTTP passkey and QUIC client-certificate doors, with the
      clock-advance test for each.
- [ ] State ledger retention, lease records included.
- [ ] Serve `urn:host:practice` with the same checks as the `ikigai` binary, and the ledger #4507
      revocation test for both doors.

## Open questions

- Whether "aggregate counts only" is compatible with a ledger, whose items necessarily name who
  wrote them. The proposal: the statement is per store, and the ledger's says what it keeps
  about authors and for how long.
- Whether a check result should be signed by the host's key, so a copy of the resource is
  evidence without asking the host again.
- Who reruns review checks, and how often, when no CI job can.
