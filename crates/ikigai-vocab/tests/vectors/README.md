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

Edit `contract_cases.json`, then regenerate and commit both files:

```text
cargo test -p ikigai-vocab --test contract_vectors -- --ignored
```

A face picks the new vectors up by copying the file again; devtools' `vector-drift`
compares the copies with this one.
