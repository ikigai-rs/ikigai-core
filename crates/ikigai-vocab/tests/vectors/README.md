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
