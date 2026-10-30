# Shared contract vectors

`contract_vectors.json` is what ikigai-core and ikigai-vocab compute for the two
identities an action has (ledger #948, #1096): **match IRIs**
(`urn:ikigai:match:{verb}:{pattern}`), **contract IRIs**
(`urn:ikigai:contract:{id}:{verb}:b3:{hex}`), the **parse** of both, and a door's
**Turtle** (`ikigai_vocab::to_turtle`). It is the one copy every face that mirrors those
functions by hand pins itself against: today ikigai-python (`ikigai.contract`, its
`to_turtle` mirror in `serve.py`) and ikigai-deno (`src/contract.ts`, `serve.ts`).

It cannot drift from core: `tests/contract_vectors.rs` recomputes every output from
`contract_cases.json` and fails if the committed file differs by a byte.

## Consuming it from another face

Copy `contract_vectors.json` **verbatim** into the face's tests (do not reformat it), and
assert against it:

| section | input | what the face must compute |
|---|---|---|
| `doors[]` | `pattern`, `description` (the JSON Meta face a Rust host deserializes) | per `actions[]`: `match` = `match_iri(verb, pattern)`, `contract` = the verb's `contract_iri`; and `turtle` = `to_turtle(description)`, byte for byte |
| `contracts[]` | `id`, `action` (the serde shape of `ikigai_core::ActionSpec`) | `contract` = `contract_iri(id, action)` |
| `matches[]` | `pattern` | per `iris[]`: `match` = `match_iri(verb, pattern)` for all five verbs |
| `parses[]` | `iri` | `match` = `parse_match_iri(iri)` as `[verb, pattern]`, `contract` = `parse_contract_iri(iri)` as `[id, verb, "b3:<hex>"]`; `null` where core refuses. `canonical` = the spelling core emits for it (`canonical_match_iri` / `canonical_contract_iri`: the parse written back by `match_iri` or as `contract_iri` writes one), `null` where neither parses |

Verbs are spelled as `ikigai_core::Verb` serializes them (`"Source"`, `"Sink"`, …). A face
that builds descriptions through its own API (decorators, families) keeps its own test that
its API produces a given `description`; this file pins only what core computes FROM one.

The `doors` named `python/…` and `deno/…` are those faces' own doors, carried here as
their JSON Meta faces, so the file is a superset of the vectors each face generated for
itself from core 0.1.92 (ikigai-python PR 28, ikigai-deno PR 22). The only output that
changed since is the ledger #1096 refusal: `urn:ikigai:match:source:%+1` parsed to
`\u0001` before and is `null` now.

Doors named `core/…` are core's own. `core/hostile-terms` (ledger #1113) is the hostile
`class`/`requires` case: a `class` or capability scope is meant to be an IRI but nothing
validates it, so core percent-encodes it when the result is a real IRI and emits a
**literal** when it is not, whatever its prefix (`https://cap> . <urn:z` is a literal);
its title and summaries carry newline, CR and tab. A face that writes `<{class}>` raw, or
chooses `<…>` for a scope by prefix alone, fails that door's `turtle` bytes.
`core/percent-names` (ledger #1345) is the `%` case: an endpoint id or input name is a
NAME, so its `%` is encoded as `%25` (`50% off` is `urn:ikigai:endpoint:50%25%20off`),
exactly as a contract IRI spells the id; a `class` or scope is an authored IRI and keeps
its escapes (`urn:cls:a%20b` stays a resource, `urn:cls:100%` is a literal). A face that
encodes names with the IRIREF set alone writes `100%%20…` for `deno/edge`'s id and fails
its `turtle` bytes. `core/bracket-names` (ledger #1361) is the rest of a NAME's encoding:
`[`, `]` and every code point outside RFC 3987's `ucschar` (DEL, a C1 control, private use, a
noncharacter) are percent-encoded in an id, an input name and a door pattern
(`http://[::1]/x` is `urn:ikigai:endpoint:http://%5B::1%5D/x`), while a `class` or scope with
an IPv6 host keeps its brackets (`<http://[::1]/cap>`); the `bracket-id` and `non-ucschar-id`
contracts and the matching `patterns` and `parses` pin the same rule. A face that stops at the
IRIREF set and `%` writes an IRI no parser accepts. `core/hash-query-names` (ledger #1390) is
`#` and `?`: a name is ONE path segment, so both are percent-encoded in an id, an input name and
a door pattern (`a#b?c#d` is `urn:ikigai:endpoint:a%23b%3Fc%23d`, the input `x#y` is
`…:input:x%23y`), while a `class` or scope keeps its fragment and query
(`<http://www.w3.org/2001/XMLSchema#string>`, `<urn:cap:hq?scope#all>`); the `hash-id` and
`query-id` contracts, the `urn:x:a#b#c` and `urn:x:{k}?q=1` patterns, and five `parses` (the
canonical, raw and lower-case spellings of a match, a raw and an encoded contract) pin the same
rule. Unlike the brackets, this respells names that parsed before (a single `#` or a `?`), so a
face that stops at the 0.1.100 set writes `urn:ikigai:endpoint:a#b`, which parses and is the
wrong resource. `tests/contract_vectors.rs` also parses every door's Turtle back, and every door
must parse.

## Two digest forms (ledger #1547)

A contract that uses none of the ArgSpec release's constructs is digested exactly as before,
under `ikigai:contract:v1` (the file's `form`); every row that predates the release is
byte-identical, the `toUpper` fixture ids included, and is the proof of that. A contract that
uses ANY of them (an input's `needs`, `list`, `values`, `examples` or an empty `"one_of": []`;
an action's `groups`, an open or envelope `arguments`, `output_from`) is digested under
`ikigai:contract:v2` (`form_v2`): v1's fields, each input encoded whole with its `needs` set,
`list` word, value notes (sorted by value: summary, tag set, needs set), empty-enumeration flag
and example set appended, the inputs sorted by their encodings; then the outputs and scopes as
in v1; then the groups, each its kind word, member set and pipe member, sorted by their
encodings; then the argument set (closed 0, open 1 + summary, envelope 2 + carrier); then
`output_from` (none 0, input 1 + name, origin 2). The `v2/*` contracts and the
`core/argspec-shape` door pin it, and `to-upper` is the renamed builtin's id. Every
variable-length field is length-prefixed as in v1 (`ikigai_core`'s `contract.rs`, `Canon`).

**Stated failure classes** (ledger #1546, ikigai-core 0.1.108): an action's `failures` (a
list of class words: `transient`, `retry-after`, `over-budget`, `denied`, `refused`,
`permanent`, `unknown`) puts it in the v2 form, and is fed LAST and only when present: the
byte `1`, then the words as a set of items (each length-prefixed, sorted by encoding,
deduplicated). An absent `failures` feeds nothing, so every contract that states none keeps
its digest, v1 or v2; an empty list (`[]`, "only the implied classes") is stated and moves
it. The `failures/*` contracts pin that, declaration order included, and the
`core/failure-classes` door pins the `ik:mayFail` triples, flat and per verb, each a
vocabulary individual (`ik:RetryAfterFailure`; `ikigai_vocab::failure_class_iri`).

**Existence sensitivity** (ledger #2176, ikigai-core 0.2.2; vectors ledger #2432): an action's
`existence_sensitive: true` puts it in the v2 form too, and is fed after everything else,
the failure classes included, only when set: the byte `2`. So a contract with both feeds the
byte `1` and its classes, then `2`; an absent flag or an explicit `false` feeds nothing and
keeps the digest. The `sensitive/*` contracts pin that: `false-is-absent` has
`unflagged`'s digest, `failures-and-flag` fixes the order of the two tails, and
`only-the-implied-failures-and-flag` (`1`, an empty set, `2`) differs from `flagged` (`2`
alone). Per verb, an explicit action's flag REPLACES the flat one, like its `requires`. The
doors pin that and the `ik:existenceSensitive true` triples: the endpoint node carries the
FLAT flag only, each action node its own verb's. `core/existence-sensitive` is flat (every
verb marked), `core/existence-sensitive-per-verb` marks only its explicit Sink beside an
unmarked flat Source, `core/existence-sensitive-replaced` is flat-marked with an explicit
Sink that is NOT, and `core/existence-sensitive-failures` states classes beside the flag.

**Inputs read as a body** (ledger #2402, ikigai-core 0.2.3; vectors ledger #2470): an
input's `reads_body: true` puts its action in the v2 form too, and is fed after existence sensitivity, only when some
input sets it: the byte `3`, then the set of those inputs' NAMES (sorted, deduplicated,
count-prefixed, each length-prefixed, as `requires` is fed). It is not part of each input's
own encoding, so no existing v2 digest moves; an absent flag or an explicit `false` feeds
nothing. The `body/*` contracts pin absent/false compatibility under both v1 and v2,
which input names are read, declaration-order independence, and composition after stated
failure classes and existence sensitivity. `core/reads-body` pins `ik:readsBody true` on
the endpoint and action input nodes, with no triple for absent or false flags.

## Lenient in, canonical out

Postel's law (ledger #1107, decided 2026-10-10). A percent escape is `%` and exactly two
hex digits; a sign (`%+1`, `%-1`) is refused. Beyond that the parse is **not**
canonical-only, and the vectors pin that so every face agrees: lower-case hex (`%7b`), an
unnecessary escape (`%41`), a character `match_iri` would have escaped (`{`), and an
upper-case contract digest each parse to the same value as the canonical spelling. That
keeps a hand-typed IRI working, and a templated door's `%7B`/`%7D` surviving a transport
that decodes once (an HTTP query string, a browser form).

Two such IRIs are different RDF terms naming one door, so **everything a face EMITS is
the canonical spelling**: a match or contract IRI taken from a caller is said back (in a
validation report, an error, a log line, a triple) as its `canonical`, never as typed.
`canonical` is a fixed point: it parses to the same `match`/`contract` and is its own
`canonical`. `tests/contract_vectors.rs` checks both for every case.

## Adding a case

Edit `contract_cases.json`, then regenerate and commit both files. A door's `description`
is deserialized as an `ikigai_core::Description`, which has no defaults for `id`, `title`,
`summary`, `verbs`, `inputs` or `outputs`: write `"title": ""`, `"summary": ""` and
`"outputs": []` even when they are empty, or the case fails to load with a missing-field
error. Each input (an `ArgSpec`) likewise needs `name`, `summary` and `required`; an
explicit action (an `ActionSpec`) needs only `verb`.

```text
cargo test -p ikigai-vocab --test contract_vectors -- --ignored
```

A face picks the new vectors up by copying the file again; devtools' `vector-drift`
compares the copies with this one.

# Shared capability vectors

`capability_vectors.json` is what ikigai-core decides about **capability coverage** and the
**declared-capability floor** (ledger #1583). ikigai-python and ikigai-deno enforce a served
door's declared `requires` themselves (ledger #1580), so each mirrors these by hand
(`ikigai.capability` and `required_scopes` in `serve.py`; `Capability` and `isDenyScope` in
`src/wire.ts`, `requiredScopes` and `denialMessage` in `src/serve.ts`), and pins itself against
a VERBATIM copy of this file instead of a hand copy of core's test tables.

It cannot drift from core: `tests/capability_vectors.rs` recomputes every output from
`capability_cases.json` and fails if the committed file differs by a byte. Each `denials` row is
computed by issuing the request through a kernel, so its text is the kernel's own; and a second
test checks every `floors` row against what the kernel ENFORCES (a caller holding nothing is
refused naming the first required scope, a caller holding exactly the list is admitted).

| section | input | what the face must compute |
|---|---|---|
| `deny_scopes[]` | `scope` | `is_deny_scope` = `is_deny_scope(scope)` |
| `coverage[]` | `capability`, `scope` | `allows` = `Capability::allows(scope)` (exact), `satisfies` = `Capability::satisfies(scope)` (a trailing `*` is "holds any scope under the prefix"; root satisfies everything) |
| `principals[]` | `capability` | `principal` = `Capability::principal()`: the ONE principal it carries, `null` for root, none, several, or a scope under `urn:cap:principal:` that names no carriable principal |
| `floors[]` | `description` (the serde shape of `ikigai_core::Description`) | per `verbs[]`: `required_scopes` = `Description::required_scopes(verb)`, for all five verbs |
| `denials[]` | `target`, `description`, `verb`, `capability` | `denied` = the text of the kernel's `Denied` for that request (core's `denial_message` for the first required scope the capability does not satisfy, in declaration order), `null` when the floor admits it |

A `capability` is `"root"` or the list of scopes a scoped capability holds (not the wire's
`{"kind": …}` form). Verbs are spelled as `ikigai_core::Verb` serializes them. The `denied`
text is the `Denied` payload, without the `denied: ` that `Display` adds; no row travels through
an alias, so none carries the alias-hop suffix (a peer never sees a hop).

Two rows record behavior worth knowing before porting: `required_scopes` deduplicates only the
UNION (an undeclared verb's floor); a declared verb's own `requires` is returned as declared,
duplicates included (`floors` `duplicate-verbs`). And an `actions[]` entry for a verb not listed
in `verbs` is ignored, as is a `Meta` action's `requires` (`action-for-undeclared-verb`,
`meta-action-is-not-gated`).

## Adding a case

Edit `capability_cases.json`, then regenerate and commit both files. A `description` needs `id`,
`title`, `summary`, `verbs`, `inputs` and `outputs` (see the contract vectors above); a
`denials` target must be an IRI.

```text
cargo test -p ikigai-vocab --test capability_vectors -- --ignored
```

A face picks the new vectors up by copying the file again; devtools' `vector-drift` compares the
copies with this one.

# Shared validation vectors

`validation_vectors.json` is what ikigai-core's ONE checker, `ActionSpec::check`, answers for a
contract and a proposed argument set (ledger #1547). ikigai-python's and ikigai-deno's `serve`
enforce the same rules (required, `one_of` per item, class plausibility, given-twice and list
joining, groups, `needs`, the empty enumeration, open and envelope argument sets), and
ikigai-mcp's tool-call check until it calls core; each pins itself against a VERBATIM copy of
this file, so four implementations stay one.

It cannot drift from core: `tests/validation_vectors.rs` recomputes every row from
`validation_cases.json` and fails if the committed file differs by a byte.

| field | what it is |
|---|---|
| `action` | the serde shape of `ikigai_core::ActionSpec` (the contract checked) |
| `args` | the proposed `[name, value]` pairs, in order, repeats kept |
| `conforms` | whether the call conforms (no violation) |
| `violations[]` | in the checker's order: `kind` (`missing`, `not-one-of`, `implausible`, `unknown`, `exactly-one`, `at-least-one`, `at-most-one`, `needs`, `given-twice`, `envelope`) and `path` (`input:{name}`, `group:{kind}:{members}` with the members sorted, `input:{name}:value:{value}`, or `null` for a name the contract does not declare) |

Rules worth knowing before porting, each a row: a binding is never missing; a group member is
GIVEN only with a non-blank value (`exactly-one/a-blank-member-is-not-given`); a list's repeats
are joined with its separator's canonical character before anything else, and a repeat of any
other name is one `given-twice`; an undeclared name is reported once however often it is given;
`needs` is one-way; an empty enumeration refuses every value; an open action admits any name but
still refuses a repeated declared one; an envelope checks its fields when the call proposes any,
its carrier otherwise, and both at once is `envelope`.

## Adding a case

Edit `validation_cases.json`, then regenerate and commit both files.

```text
cargo test -p ikigai-vocab --test validation_vectors -- --ignored
```

A face picks the new vectors up by copying the file again; devtools' `vector-drift` should
compare the copies with this one (it does not know this pair yet).

# Shared failure-class vectors

`failure_class_vectors.json` is the one table of how a typed error fails (ledger #1546, #200):
for every `ikigai_core::Error` variant, what core computes (its `kind` word, its failure `class`,
whether it is `transient`, its `display` form, and `retry_after_ms`, the delay in whole
milliseconds rounded up) beside the DECIDED rows every face pins (decision 4 of ledger #1546):
the wire v9 `wire_tag` and the HTTP edge's `http.status` and `http.retry_after`. The Rust wire
suite and the edge's `error_status` (ikigai-cli), ikigai-python and ikigai-deno each pin
themselves against a VERBATIM copy of it.

It cannot drift from core: `tests/failure_class_vectors.rs` recomputes every row from
`failure_class_cases.json` and fails if the committed file differs by a byte. The decided columns
are written there as two small tables (`wire_tag`, `http_status`) that panic on a variant they do
not name, so a variant added to core cannot reach the file without a decided tag and status.

| field | what it is |
|---|---|
| `error` | the case: `kind` names the variant, then its payload (`iri`, `name`, `detail`, `message`, `depth` and `target`, `verb` as `ikigai_core::Verb` serializes it, `delay_us` in microseconds) |
| `kind` | `Error::kind` |
| `class` | `Error::class`, one of `transient`, `retry-after`, `over-budget`, `denied`, `refused`, `permanent`, `unknown` |
| `transient` | `Error::is_transient`: true for `transient` alone; `retry-after` is NOT transient |
| `display` | the `Display` form, which is what a v8 peer receives inside `Endpoint` for a variant its wire has no tag for |
| `retry_after_ms` | `Error::retry_after_millis`: the wire form of the delay, rounded UP, `null` without one |
| `wire_tag` | the v9 discriminant: 0-8 as today, then `depth-exceeded` 9, `retry-after` 10, `over-budget` 11, `refused` 12, `unsupported` 13 |
| `http.status` | the edge's status: 404 unresolved and not-found, 400 the argument errors, 403 denied, 409 conflict, 504 timeout, 503 unavailable, 429 retry-after, 422 over-budget, 502 refused, 500 endpoint and depth-exceeded, 405 an unsupported verb, 501 an unsupported facility |
| `http.retry_after` | the `Retry-After` header in delay-seconds, rounded up and NEVER 0; `null` when no header is sent |

`classes[]` lists every class `word` with its vocabulary individual `iri`
(`ikigai_vocab::failure_class_iri`).

Rules worth knowing before porting, each a row: a sub-millisecond delay rounds UP to the next
millisecond (`retry-after/a-fraction-of-a-millisecond-rounds-up` displays `2 ms`); a zero delay
still sends `Retry-After: 1`; a `retry-after` with no delay is a 429 with no header and displays
`retry later: …`; `unsupported` is one class and two statuses, decided by whether it names a verb.

## Adding a case

Edit `failure_class_cases.json`, then regenerate and commit both files.

```text
cargo test -p ikigai-vocab --test failure_class_vectors -- --ignored
```

A face picks the new vectors up by copying the file again; devtools' `vector-drift` should
compare the copies with this one (it does not know this pair yet).
