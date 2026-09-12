#!/usr/bin/env python3
"""Regenerate src/context.jsonld from src/vocabulary.ttl.

The JSON-LD @context is a projection of the vocabulary: every ns# term mapped to
its short name, with datatype / @id coercions inferred from each property's
declared rdfs:range — and ONLY from the range: an IRI-valued property declares
`rdfs:range rdfs:Resource` (or an ik: class) in the vocabulary rather than being
special-cased here. Run this whenever vocabulary.ttl changes — two tests fail if
the two drift apart (`context_covers_every_vocabulary_term` compares the term
names, `context_generator_sees_no_drift` runs `--check`, which also catches a
changed coercion).

    python3 crates/ikigai-vocab/context.gen.py          # rewrite context.jsonld
    python3 crates/ikigai-vocab/context.gen.py --check  # exit 1 if it would change

Fails (exit 1) if vocabulary.ttl declares the same term twice — a later
declaration would otherwise silently overwrite the earlier one's mapping.

No dependencies (parses the Turtle directly). Idempotent.
"""
import json
import pathlib
import re
import sys

SRC = pathlib.Path(__file__).resolve().parent / "src"
NS = "https://ikigai-rs.dev/ns#"
XSD = "http://www.w3.org/2001/XMLSchema#"


def coercion(name: str, rng: str | None):
    """One property's JSON-LD term entry, from its declared rdfs:range and nothing else.

    An IRI-valued range (rdfs:Resource, or an ik: class) coerces to @id. Every other
    xsd datatype coerces to ITSELF rather than being enumerated here, so a range this
    file has never seen (xsd:nonNegativeInteger, xsd:positiveInteger, xsd:anyURI …)
    is carried the day the vocabulary declares it — the point of driving the context
    off the range is that the vocabulary, not the generator, decides.

    xsd:string, rdfs:Literal and an undeclared range stay plain terms: a JSON string
    is already what they mean, and a coercion would only add noise.
    """
    if rng == "rdfs:Resource" or (rng and rng.startswith("ik:")):
        return {"@id": f"ik:{name}", "@type": "@id"}
    if rng and rng.startswith("xsd:") and rng != "xsd:string":
        return {"@id": f"ik:{name}", "@type": rng}
    return f"ik:{name}"


def build_context(ttl: str) -> dict:
    ctx = {"ik": NS, "xsd": XSD}
    entries: dict[str, object] = {}
    declared_at: dict[str, int] = {}
    line = 1
    # Terms are blank-line-separated paragraphs (`ik:Name a rdf:Property ; … .`).
    # The capturing split keeps the separators so `line` stays accurate for
    # duplicate reporting.
    for chunk in re.split(r"(\n\s*\n)", ttl):
        m = re.match(r"\s*ik:(\w+)\s+a\s+(rdf:Property|rdfs:Class)\b", chunk)
        if not m:
            line += chunk.count("\n")
            continue
        name, kind = m.group(1), m.group(2)
        decl_line = line + chunk[: m.start(1)].count("\n")
        if name in declared_at:
            sys.exit(
                f"context.gen.py: duplicate declaration of ik:{name} in "
                f"vocabulary.ttl (lines {declared_at[name]} and {decl_line}) — "
                f"the later one would silently overwrite the earlier mapping"
            )
        declared_at[name] = decl_line
        if kind == "rdfs:Class":
            entries[name] = f"ik:{name}"
        else:
            rm = re.search(r"rdfs:range\s+(\S+)", chunk)
            rng = rm.group(1).rstrip(";.").strip() if rm else None
            entries[name] = coercion(name, rng)
        line += chunk.count("\n")
    for key in sorted(entries):
        ctx[key] = entries[key]
    return {"@context": ctx}


def render(ttl: str) -> str:
    return json.dumps(build_context(ttl), indent=2, ensure_ascii=False) + "\n"


def main(argv: list[str]) -> None:
    ttl = (SRC / "vocabulary.ttl").read_text()
    out = render(ttl)
    target = SRC / "context.jsonld"
    if argv[1:] == ["--check"]:
        current = target.read_text() if target.exists() else ""
        if current != out:
            sys.exit(
                "context.gen.py --check: src/context.jsonld is stale — regenerate it: "
                "python3 crates/ikigai-vocab/context.gen.py"
            )
        print("context.jsonld is up to date")
        return
    if argv[1:]:
        sys.exit(f"context.gen.py: unknown arguments {argv[1:]} (only --check is accepted)")
    target.write_text(out)
    print(f"wrote context.jsonld ({out.count(chr(10))} lines)")


if __name__ == "__main__":
    main(sys.argv)
