#!/usr/bin/env python3
"""Regenerate src/context.jsonld from src/vocabulary.ttl.

The JSON-LD @context is a projection of the vocabulary: every ns# term mapped to
its short name, with datatype / @id coercions inferred from each property's
declared rdfs:range. Run this whenever vocabulary.ttl changes — a test
(`context_covers_every_vocabulary_term`) fails if the two drift apart.

    python3 crates/ikigai-vocab/context.gen.py

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
# Properties whose value is an IRI but whose range is left undeclared (or a plain
# string in the vocab) — coerce them to @id so a string value is read as a reference.
IRI_PROPS = {"endpoint", "class"}


def coercion(name: str, rng: str | None):
    if rng == "xsd:integer":
        return {"@id": f"ik:{name}", "@type": "xsd:integer"}
    if rng == "xsd:boolean":
        return {"@id": f"ik:{name}", "@type": "xsd:boolean"}
    if rng == "xsd:decimal":
        return {"@id": f"ik:{name}", "@type": "xsd:decimal"}
    if rng == "xsd:dateTime":
        return {"@id": f"ik:{name}", "@type": "xsd:dateTime"}
    if rng == "rdfs:Resource" or (rng and rng.startswith("ik:")) or name in IRI_PROPS:
        return {"@id": f"ik:{name}", "@type": "@id"}
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


def main() -> None:
    ttl = (SRC / "vocabulary.ttl").read_text()
    out = json.dumps(build_context(ttl), indent=2, ensure_ascii=False) + "\n"
    (SRC / "context.jsonld").write_text(out)
    print(f"wrote context.jsonld ({out.count(chr(10))} lines)")


if __name__ == "__main__":
    main()
