# Claim asserters: who asserted each claim, and in what standing

**Status:** proposed (ledger #4511). A design note only: nothing in the vocabulary, the kernel or
any module changes here, and the terms below are a draft. Publishing them is a vocabulary change,
so it needs a vocab publish and an `ikigai-rs.dev/ns` deploy, which are Brian's. Written on
2026-11-27 against core main at
[`b09fcea`](https://github.com/ikigai-rs/ikigai-core/tree/b09fcea6c07568e73bb566cef285db7535d815a0),
read against ikigai-judge, ikigai-credential, ikigai-knowledge, ikigai-log and gonk's hand-backs
(commits below). The source is a policy paper on accountable agents. Its point: each claim should
say whether a directory (a third party that checked it), the principal (the person or
organization responsible) or the agent itself asserted it.

Evidence:
[`crates/ikigai-vocab/tests/asserter_roles.rs`](../../crates/ikigai-vocab/tests/asserter_roles.rs).
`today_the_standing_is_not_in_the_graph` pins the gap on the published vocabulary. The other
three tests parse the draft below out of this note and hold it to the conventions
`vocabulary.ttl` states, so a publish copies terms that already pass them.

## The question

ikigai emits claims: a hand-back says work is done and cites its evidence, ikigai-judge records a
verdict, ikigai-credential issues and verifies credentials, ikigai-knowledge cites a passage for
an answer, and ikigai-log records what happened. A reader of any of them needs two facts:

1. **Who** asserted it: an agent IRI. PROV-O already says this with `prov:wasAttributedTo`, and
   the layer host, the grant and credential records, and drafting attempts use it.
2. **In what standing** they asserted it: as the agent doing the work, as the principal
   responsible for it, as a person reviewing someone else's claim, as a deterministic check,
   as a model, or as a third party. Nothing says this today.

The second fact cannot be derived from the first. The same person is the principal on a brief
they write and a reviewer on a verdict they record about someone else's work. The same machine
is the acting agent when it hands work back and a deterministic check when it runs CI. The test
`today_the_standing_is_not_in_the_graph` builds exactly that graph: two claims, one person, the
same `prov:wasAttributedTo` and the same agent type, and nothing that tells the standings apart.
The vocabulary declares no term for it and types nothing a `prov:Role`.

The standing is what a reader acts on. A hand-back's evidence is the agent's own claim until a
dispatcher verifies it. A model's verdict yields to a person's. A credential a third party
signed means something a self-issued one does not.

## Decision: one property, six values

Draft **`ik:asserterRole`**: the standing in which a claim's asserter made it, as one concept of
the scheme **`ik:AsserterRoles`**.

| Concept | Notation | The asserter is | Typical claim |
|---|---|---|---|
| `ik:AgentAsserter` | `agent` | the agent doing the work, speaking about its own work | a hand-back and its evidence, a progress report, an agent's log entry |
| `ik:PrincipalAsserter` | `principal` | the person or organization responsible, on their own authority | a brief, a grant, a credential an organization issues, a waiver |
| `ik:ReviewerAsserter` | `reviewer` | a person judging a claim or an item someone else produced | an ikigai-judge person verdict, a published review finding, a dispatcher's close |
| `ik:CheckAsserter` | `check` | a deterministic procedure: same inputs, same answer | a judge mapping (rule) verdict, a CI run, SHACL validation, a signature or quote check |
| `ik:ModelAsserter` | `model` | a statistical or generative model | a judge model or bayes verdict, a review finding, an LLM-drafted answer |
| `ik:ThirdPartyAsserter` | `third-party` | a party outside the principal's organization that checked it (a directory, an external issuer) | a credential another organization signed, an external registry's attestation |

### The drafted terms

```turtle
ik:asserterRole a rdf:Property ;
    rdfs:range skos:Concept ;
    rdfs:label "asserterRole" ;
    rdfs:comment "The standing in which a claim's asserter made it, as one concept of ik:AsserterRoles: the acting agent about its own work, the principal responsible, a person reviewing, a deterministic check, a model, or a third party. prov:wasAttributedTo names WHO asserted the claim; this names in what standing, which the agent's type cannot say: one person is the principal on a brief and a reviewer on a verdict, one machine the acting agent on a hand-back and a check on a CI run. At most one per claim. A claim confirmed, overruled or verified by someone else is not restamped: the confirmation is its own claim, with its own role, that names the first (prov:wasDerivedFrom), so the agent's claim and the reviewer's stay one hop apart and both survive. Absent means UNSTATED, never agent: a reader treats an unstated role as the weakest it accepts. Set by whoever records the claim and knows the standing (a host stamping prov:wasAttributedTo, a judge recording a verdict), never taken from the asserter's own say-so, as a layer host refuses a sent ik:seq. The shorthand for a PROV qualified attribution: C ik:asserterRole R with C prov:wasAttributedTo A says what C prov:qualifiedAttribution [ prov:agent A ; prov:hadRole R ] says, so each concept is a prov:Role too. The vocabulary states no precedence among the roles; a reader that must pick between claims states its own (ikigai-judge's person, then rule, then model is one). No domain: a hand-back, a verdict, a credential, a citation and a log entry all carry it, and a domain would type each as the others." .

ik:AsserterRoles a skos:ConceptScheme ;
    rdfs:label "AsserterRoles" ;
    rdfs:comment "The standings a claim's asserter can make it in: the values of ik:asserterRole. Closed and enumerated here, unlike ik:severity's values, because the readers are every module rather than the one that serves them: a dispatcher filtering hand-backs, a judge weighing verdicts and an audit over the log ask the same question of different emitters. A configuration or a line face names one by its skos:notation (ledger #4511)." ;
    skos:prefLabel "Asserter roles"@en ;
    skos:hasTopConcept ik:AgentAsserter , ik:PrincipalAsserter , ik:ReviewerAsserter ,
        ik:CheckAsserter , ik:ModelAsserter , ik:ThirdPartyAsserter .

ik:AgentAsserter a skos:Concept , prov:Role ;
    skos:inScheme ik:AsserterRoles ;
    skos:topConceptOf ik:AsserterRoles ;
    skos:notation "agent" ;
    skos:prefLabel "Agent"@en ;
    skos:definition "The agent doing the work asserted it about its own work: a hand-back, its evidence, a progress report. Its own claim, until someone in another standing confirms it."@en .

ik:PrincipalAsserter a skos:Concept , prov:Role ;
    skos:inScheme ik:AsserterRoles ;
    skos:topConceptOf ik:AsserterRoles ;
    skos:notation "principal" ;
    skos:prefLabel "Principal"@en ;
    skos:definition "The person or organization responsible asserted it on their own authority: a brief, a grant, a credential they issue, a waiver."@en .

ik:ReviewerAsserter a skos:Concept , prov:Role ;
    skos:inScheme ik:AsserterRoles ;
    skos:topConceptOf ik:AsserterRoles ;
    skos:notation "reviewer" ;
    skos:prefLabel "Reviewer"@en ;
    skos:definition "A person asserted it after judging a claim or an item someone else produced: a person verdict, a published finding, a dispatcher's close of a hand-back."@en .

ik:CheckAsserter a skos:Concept , prov:Role ;
    skos:inScheme ik:AsserterRoles ;
    skos:topConceptOf ik:AsserterRoles ;
    skos:notation "check" ;
    skos:prefLabel "Check"@en ;
    skos:definition "A deterministic procedure asserted it, one that gives the same answer for the same inputs: a rule or mapping, a CI run, a SHACL validation, a signature or quote check."@en .

ik:ModelAsserter a skos:Concept , prov:Role ;
    skos:inScheme ik:AsserterRoles ;
    skos:topConceptOf ik:AsserterRoles ;
    skos:notation "model" ;
    skos:prefLabel "Model"@en ;
    skos:definition "A statistical or generative model asserted it: a language model's answer or finding, a classifier's verdict. ik:model names which model, when known."@en .

ik:ThirdPartyAsserter a skos:Concept , prov:Role ;
    skos:inScheme ik:AsserterRoles ;
    skos:topConceptOf ik:AsserterRoles ;
    skos:notation "third-party" ;
    skos:prefLabel "Third party"@en ;
    skos:definition "A party outside the principal's organization that checked the claim asserted it: a directory, an external credential issuer, a registry."@en .
```

The JSON-LD `@context` gains `asserterRole` coerced to `@id` (the range is `skos:Concept`), and
the three terms ride the existing `context.gen.py` regeneration when they are published.

### Why these choices

- **A role, not a class of agent.** The standing belongs to the claim, not to the agent. A type
  on the agent (`foaf:Person`, `prov:SoftwareAgent`, `ik:Machine`) stays what it is, and the
  principals graph stays the only place that says who a principal is.
- **A flat property, and the qualified form for PROV readers.** PROV-O can already say this:
  `prov:qualifiedAttribution` to a `prov:Attribution` with `prov:agent` and `prov:hadRole`. That
  costs a node per claim, which this vocabulary would have to skolemize, since it writes no blank
  nodes. It also costs a reader filtering "everything a model said" a second hop, and a layer's
  egress context a second term to let through. The flat property is one triple. Typing each
  concept a `prov:Role` lets an emitter that wants the qualified form use the same IRIs, and the
  equivalence is stated in the comment rather than as an OWL axiom nobody here runs.
- **At most one per claim, and no restamping.** A verified hand-back is two claims: the agent's
  and the verifier's. This is the pattern `ik:Finding` already follows: a model's finding is
  never rewritten, and publishing mints a separate annotation `prov:wasDerivedFrom` it. Restamping
  the agent's claim as `reviewer` would erase the fact a calibration query needs: what the agent
  said before anyone checked.
- **Absent is unstated.** As with `ik:reduceMotion`, absence is not a value. Records written
  before the term exists carry none, and a reader must not read them as any role.
- **Stamped by the recorder, never sent.** An asserter cannot raise its own standing. The layer
  host already refuses a sent `prov:wasAttributedTo`, `dcterms:created` or `ik:seq`, and it
  refuses a sent `ik:asserterRole` the same way.
- **No precedence in the vocabulary.** Readers disagree on order: a judge ranks a person over a
  rule, while a security gate ranks a signature check over a person's say-so. The vocabulary
  names the standings. Each reader declares its own order and says so.
- **Enumerated here.** `ik:severity` keeps its values in the code that serves them, because one
  module both writes and reads them. These values are read across every module, like
  `ik:LayerKinds`, so the vocabulary is where they agree.
- **Names not taken.** *Issuer* is a W3C Verifiable Credential's `issuer`, and in the kernel it
  is the request issuer every sub-request passes through. *Source* is a verb. *Attribution* is
  PROV's. *Asserter role* collides with none of them, and the test
  `no_drafted_term_collides_with_a_published_one` checks the draft against every published term,
  in any case.

## Mapping onto what exists

Read on 2026-11-27 at these main commits: ikigai-judge
[`c007248`](https://github.com/ikigai-rs/ikigai-judge/tree/c00724802d8071c388066c159f72fbca3199edd2),
ikigai-log [`d60caf2`](https://github.com/ikigai-rs/ikigai-log/tree/d60caf2b22aaa02572190832fefcc395419074e9),
ikigai-credential [`95ce0c3`](https://github.com/ikigai-rs/ikigai-credential/tree/95ce0c3cdbd2da63673a07fe091eb2d97d5783c0),
ikigai-knowledge [`1a5ae79`](https://github.com/ikigai-rs/ikigai-knowledge/tree/1a5ae793157504c3ea17e130455cc3f02c9d82f0),
ikigai-gonk [`1d97a1d`](https://github.com/ikigai-rs/ikigai-gonk/tree/1d97a1dbfde2d0e5bfd0fe19e2d5a8add442abb0),
ikigai-layers [`48c7c69`](https://github.com/ikigai-rs/ikigai-layers/tree/48c7c6952fb09eba60b25fef186d0778340e73d3)
and ikigai-ledger [`6739ec6`](https://github.com/ikigai-rs/ikigai-ledger/tree/6739ec6e295dbe858a572b789d1eb7258f0e75ae).
None of them defines a term for this today, and none uses `assertedBy`, `issuerKind`,
`verdictSource` or `sourceKind`.

### PROV-O

`prov:wasAttributedTo` stays the who. `ik:asserterRole` is the shorthand for the role of a
qualified attribution (`prov:qualifiedAttribution` / `prov:hadRole`), and each concept is a
`prov:Role` so the long form can use the same IRIs. It does not replace `prov:wasAssociatedWith`
(an agent's part in an activity) or `prov:actedOnBehalfOf` (delegation, which the principals
graph and the log already carry). The role answers "in what standing did they say it", and
delegation answers "for whom were they acting". An agent's hand-back is `agent` even though the
agent acts for a principal. The principal's own say-so would be a separate claim, `principal`.

### ikigai-judge: person > rule > model

The judge's `Producer` enum (`src/lib.rs`) orders verdicts: a person (unshared, then shared),
then a mapping, then a model, then a stale model, then bayes. `decide()` takes the first that
answers. The kinds are Rust strings today. A stored person verdict names its maker with
`dcterms:creator`, a model verdict with `prov:wasAssociatedWith` and `prov:wasGeneratedBy` a run.
The "why" face gives each ranked verdict `schema:agent "<producer>"` as a string literal.

| Producer | `ik:asserterRole` |
|---|---|
| person (unshared or shared) | `reviewer` |
| mapping | `check` |
| model, stale model | `model` (a stale one is still a model's claim; staleness is the judge's own fact) |
| bayes | `model` |

The precedence stays the judge's. It is a reader's declared order over these roles, with its
finer distinctions kept in its own terms: shared vs. unshared, fresh vs. stale, confident bayes.
The disagreement signal (`disagreements()`, served at `urn:iki:judge:disagreements:{q}`) is
claims in another standing that differ from the deciding one, which the role makes queryable
in RDF once the signal has an RDF face.

### ikigai-log: the principal column

The principal is the `principal=` key (`src/vocabulary.rs`), bound to `log:onBehalfOf`. On
transreption, each entry is `prov:wasAssociatedWith` the host instance, the segment is
`prov:wasAttributedTo` it, and the principal becomes a `prov:qualifiedDelegation`. So the log
already says who acted and for whom. It does not say in what standing the entry's claims were
made, and `principal=` is not that: it is the delegation, the "for whom". An entry the host
writes about its own work is `agent`. An entry recording a check's result (a validation, a
signature verify) is `check`. `log:credentialKind` is the precedent for a closed set of
individuals in the log vocabulary.

### ikigai-credential

A Verifiable Credential's `issuer` (a `Profile` with a `did:web` id) and the log's
`credential:Issuance` with `credential:issuer` say who signed. When this host issues for its own
organization, the issuance is `principal`. A credential this host verifies that another
organization issued is `third-party`. `urn:credential:verify` answers a plain-text verdict and
records nothing. Its result is a `check` claim (the signature and status checked out), separate
from the credential's own claims, whose standing is the issuer's.

### ikigai-knowledge: citations

A trail answer's claims are `oa:Annotation` nodes whose target is a passage, with a
`TextQuoteSelector` and a `TextPositionSelector`. The model behind a trail answer is not in its
Turtle. Drafts carry `knowledge:model` on their generating activity. Person verdicts on an answer
are `prov:wasAttributedTo` the person, with a `knowledge:decision`. So a cited claim is `model`.
A quote match that proves the selector's text is in the passage is a separate `check` claim,
and it proves the quote, not the claim it supports. A person's verdict is `reviewer`.

### gonk, layers and the ledger: hand-backs

gonk's taker writes an `ik:Handback` with `ik:offer`, `ik:addressedTo` and `ik:evidence`. The
layer host stamps `prov:wasAttributedTo` (the dropper), `dcterms:created` and `ik:seq`, and
refuses each if sent. gonk's mirror writes a ledger comment with `ledger:handBack`, and the item
waits on `ledger:awaiting <urn:iki:ledger:awaiting:dispatcher>`. The dispatcher's check has no
term: it closes the item. So a hand-back and its evidence are `agent` claims, and nothing records
the dispatcher's verification as a claim of its own.

The closest existing design is ikigai-ledger's attribution (`src/attribution.rs`), modeled on
git's committer and author. The door stamps `ledger:committer`. `ledger:author` is verified or
claimed, and `ledger:authorSource` says how (`committer`, `delegate` or `import`). That answers
a neighboring question: how sure the record is about who wrote it. `ik:asserterRole` answers in
what standing they wrote it. Both can sit on one record.

## Tasks per emitter

Each task waits for the terms' publish. Each lands in its own repository and its own pull
request, with a test that the term is emitted.

- **ikigai-vocab (this repository):** copy the drafted block into `vocabulary.ttl` in the
  provenance section, regenerate `context.jsonld`, and flip
  `today_the_standing_is_not_in_the_graph` to read the terms from `VOCABULARY`. Then Brian's
  vocab publish and `/ns` deploy.
- **ikigai-layers:** stamp `ik:asserterRole` on every tuple and entry from the participant's
  standing in the layer: a taker's `ik:Progress`, `ik:Release` and `ik:Handback` are `agent`. An
  `ik:Offer` or `ik:Brief` from the principal is `principal`. Refuse a sent role, as with
  `ik:seq`. Add `asserterRole` to each layer's egress context where a reader needs it.
- **ikigai-gonk:** keep the taker's hand-back `agent`, and carry the role into the mirror's ledger
  comment. Record a dispatcher's verification as its own claim that `prov:wasDerivedFrom` the
  hand-back: `check` when it rests on CI, `reviewer` when a person closed it. Today the close is
  the only trace.
- **ikigai-ledger:** surface the role beside `ledger:authorSource` on comments and closes, and
  let `ledger:awaiting` name the standing it waits for (a `check` or a `reviewer`).
- **ikigai-judge:** map `Producer` to the role, as in the table above, on stored verdicts and on
  the "why" face. That face keeps `schema:agent` for the producer name and gains the role IRI.
  Give the disagreement signal an RDF face that carries both roles.
- **ikigai-log:** add an `asserter=` key bound to `ik:asserterRole`, written by notation (`agent`,
  `check`, ...). Default it to `agent` only for lines the host writes about its own work, and
  leave it absent otherwise. Keep `principal=` as the delegation.
- **ikigai-credential:** stamp `principal` on issuances for the host's own organization and
  `third-party` on credentials it verifies from others. Record a verification as a `check`
  claim with its result instead of only answering a sentence.
- **ikigai-knowledge:** stamp `model` on a trail answer's claims, and name the model there as
  drafts already do (`ik:model` or `knowledge:model`). Stamp `check` on a quote-match result
  and `reviewer` on person verdicts.
- **ikigai-browse (review findings):** `ik:Finding` is `model`, and the decision and the
  published `oa:Annotation` are `reviewer`. That is the separation `ik:severity` already
  depends on, now stated.

## What a reader does with it

- **Unverified is the default for an agent's claim.** A dispatcher reading a hand-back treats
  `agent` evidence as a claim to check, not a fact. It closes only on a claim in another standing
  that names the evidence: a `check` (CI green on the cited commit) or a `reviewer` (a person's
  close). The dispatcher records its own close as that second claim.
- **Pick by declared precedence, and show the disagreement.** A reader holding several claims
  about one subject applies its declared order and keeps the others. ikigai-judge's disagreement
  signal is this: a `model` verdict that differs from a `reviewer` one is data about the model.
- **Calibration is a query.** "How often is the agent's hand-back right?" counts `agent` claims
  against the `reviewer` and `check` claims derived from them. "How often does the model agree
  with people?" does the same for `model` against `reviewer`. Neither is possible while a
  confirmation overwrites what it confirms.
- **Unstated is the weakest.** A reader that requires a standing (for example, only `reviewer`
  or `third-party` claims may close an item) refuses a claim that states none, and says so by
  name.
- **The role is not authority.** Like a door fact, a role is an observation. It never grants a
  capability, and a reader that gates on it gates through its own policy, which is a capability
  check like any other.

## Upgrade from older versions

Nothing persisted or emitted changes with this note. When the terms are published:

- Records written before carry no `ik:asserterRole` and stay valid. Readers treat them as
  unstated, never as `agent`. No backfill is required. A host may backfill a role where its
  own records make the standing certain (a judge verdict's recorded kind), as a new claim that
  `prov:wasDerivedFrom` the old one, never by rewriting it.
- A layer whose egress context is an allowlist lets the term through only once its context lists
  `asserterRole`. Until then, readers on the far side see unstated roles.
- The vocabulary's `owl:versionInfo` moves with the crate release that publishes the terms, and
  the `@context` is regenerated with them.

## Open decisions for Brian

1. The names: `ik:asserterRole` and the `…Asserter` concepts, or a shorter `ik:assertedAs`.
2. Whether `third-party` should split into a directory (checked the subject) and an external
   issuer (asserted about the subject). The paper's directory is the first, a VC issuer the
   second. This note keeps one value until an emitter needs the distinction.
3. Whether the layer host should refuse a sent `ik:asserterRole` (as drafted) or admit it from a
   participant whose grant allows a stated standing, for a reviewer writing through a tool.
