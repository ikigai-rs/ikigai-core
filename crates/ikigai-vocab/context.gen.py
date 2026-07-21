#!/usr/bin/env python3
"""Regenerate src/context.jsonld from src/vocabulary.ttl.

The JSON-LD @context is a projection of the vocabulary: every ns# term mapped to
its short name, with datatype / @id coercions inferred from each property's
declared rdfs:range. Run this whenever vocabulary.ttl changes — a test
(`context_covers_every_vocabulary_term`) fails if the two drift apart.

    python3 crates/ikigai-vocab/context.gen.py

No dependencies (parses the Turtle directly). Idempotent.
"""
import json
import pathlib
import re

SRC = pathlib.Path(__file__).resolve().parent / "src"
NS = "https://ikigai-rs.dev/ns#"
XSD = "http://www.w3.org/2001/XMLSchema#"
# Properties whose value is an IRI but whose range is left undeclared (or a plain
# string in the vocab) — coerce them to @id so a string value is read as a reference.
IRI_PROPS = {"endpoint", "class"}


def coercion(name: str, rng: str | None):
    if rng == "xsd:integer":
        return {"@id": f"ik:{name}", "@type": "xsd:integer"}
    if rng == "xsd:boolean":
        return {"@id": f"ik:{name}", "@type": "xsd:boolean"}
    if rng == "rdfs:Resource" or (rng and rng.startswith("ik:")) or name in IRI_PROPS:
        return {"@id": f"ik:{name}", "@type": "@id"}
    return f"ik:{name}"


def build_context(ttl: str) -> dict:
    ctx = {"ik": NS, "xsd": XSD}
    entries: dict[str, object] = {}
    # Terms are blank-line-separated paragraphs (`ik:Name a rdf:Property ; … .`).
    for para in re.split(r"\n\s*\n", ttl):
        m = re.match(r"\s*ik:(\w+)\s+a\s+(rdf:Property|rdfs:Class)\b", para)
        if not m:
            continue
        name, kind = m.group(1), m.group(2)
        if kind == "rdfs:Class":
            entries[name] = f"ik:{name}"
        else:
            rm = re.search(r"rdfs:range\s+(\S+)", para)
            rng = rm.group(1).rstrip(";.").strip() if rm else None
            entries[name] = coercion(name, rng)
    for key in sorted(entries):
        ctx[key] = entries[key]
    return {"@context": ctx}


def main() -> None:
    ttl = (SRC / "vocabulary.ttl").read_text()
    out = json.dumps(build_context(ttl), indent=2, ensure_ascii=False) + "\n"
    (SRC / "context.jsonld").write_text(out)
    print(f"wrote context.jsonld ({out.count(chr(10))} lines)")


if __name__ == "__main__":
    main()
