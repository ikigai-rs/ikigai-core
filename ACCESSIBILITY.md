# Accessibility

What `ikigai-core` emits that people read, the rules that output follows, how it is tested, and
how to report a barrier. It is a library: almost everything it writes reaches a person through a
host (a REPL, an HTTP door, an editor) that owns the page or terminal it lands on. It is not a
conformance claim. Adapted from the [ACCESSIBILITY.md
template](https://github.com/mgifford/ACCESSIBILITY.md) (MIT).

| Field | Value |
|---|---|
| Target | WCAG 2.2 Level AA, for any page that presents this output; this repository writes text and RDF, and three SVG figures in its design notes |
| Status | Evaluation in progress. No conformance claim. |
| Owner | the maintainers of this repository |
| Report a barrier | [a GitHub issue](https://github.com/ikigai-rs/ikigai-core/issues/new?labels=accessibility) with the `accessibility` label; anything security-sensitive through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-core/security/advisories/new) |
| Last reviewed | 2026-10-31 |

## What it emits that people read

- **Error messages**: the `Display` of `Error` (`crates/ikigai-core/src/error.rs`), which every
  host prints or serves, for example `missing required argument `name``, `invalid argument
  `as`: …`, `no endpoint resolved for urn:…`, `denied: …`, `not found: …`; and the `Display` of
  the smaller error types (`AliasRefusal` and `AliasParseError` in `alias.rs`,
  `DeclarationError` in `declare.rs`, `TemplateError` in `grammar.rs`, `CacheConfigError`,
  `ContentIdError`, `PrincipalError`, `DynamicRefusal`).
- **Validation messages**: each `Violation` of `urn:kernel:validate` (`src/check.rs`) carries a
  message that names the inputs it is about (for example, which of a group is required, and what
  was given).
- **The kernel's text faces**: the `text/plain` answers of `urn:kernel:aliases`, `cached`,
  `uncached`, `cut`, `threads`, `explain` and the dynamic space's status (`src/kernel_ops.rs`,
  `src/explain.rs`, `src/uncached.rs`, `src/dynamic.rs`): plain lines, some laid out as a
  label column and a value column.
- **Vocabulary prose**: the `rdfs:label` and `rdfs:comment` text in
  `crates/ikigai-vocab/src/vocabulary.ttl` and `shapes.ttl`, published at
  <https://ikigai-rs.dev/ns> and read by people browsing the vocabulary.
- **Design figures**: three SVG pictures in `docs/design/img/`, shown in
  `docs/design/spaces-as-named-graphs.md`. They are documentation, not output of the crates.
- **A PDF walkthrough**, built on demand by `tools/walkthrough/generate.py` and never committed.

Not this repository's: the pages, terminal screens and colors a host draws around this text, and
the Turtle, HTML or JSON faces other modules render from the kernel's descriptions.

## The rules that output follows

Verified by reading the source on 2026-10-31:

- An error names what it is about: a missing or invalid argument by its name, an unresolved
  target and a depth refusal by the IRI. The first words say the kind of failure in words
  (`denied:`, `not found:`, `conflict:`, `timeout:`), never a code alone.
- A kernel operation asked for a face it does not serve refuses with a sentence that names the
  face it was asked for, the operation, and the faces it does serve.
- No color, no terminal escape and no markup is written: every message and face is plain text.
- What this repository writes uses American spelling.
- Each design figure is `role="img"` with a `<title>` and a `<desc>` that states what the picture
  shows, and every label in it is real `<text>`.

## How it is tested

- `crates/ikigai-core/tests/kernel_faces.rs`
  (`single_face_operations_refuse_unsupported_as`, `two_face_operations_refuse_unsupported_as`)
  pins that a refused face names the argument, the face asked for, the operation and the faces
  served.
- The doctests on `Error::Conflict`, `RetryAfter`, `OverBudget` and `Refused` in `error.rs` pin
  their displayed text, and `crates/ikigai-vocab/tests/failure_class_vectors.rs` pins the
  `display` form of every case in `tests/vectors/failure_class_vectors.json`.
- `crates/ikigai-core/tests/american_spelling.rs` scans the docs, the Rust sources and the
  vocabulary's prose for British spellings.
- Nothing tests the design figures.
- **No automated accessibility checker, browser or assistive technology has been used on this
  output.**

## Known limitations

Found by reading the source on 2026-10-31, not by testing:

- `Error::Endpoint` displays as `endpoint error: …` with whatever text the endpoint gave; it names
  no endpoint or target unless that text does, and most errors say what went wrong but not what
  to do next.
- Messages wrap argument names in backticks (`` `name` ``), Markdown punctuation that a screen
  reader may read aloud in a plain-text face.
- The design figures use fixed light colors on a transparent background with no dark-scheme
  variant, so on a dark page the text drawn outside a box may lose contrast; their contrast has
  not been measured. Their root `<svg>` does not reference the `<title>` and `<desc>` with
  `aria-labelledby`, and states no language.
- The walkthrough PDF sets no document title or language and is not tagged, and its code is set
  in 7.2-point type.

## Contributing

Read this file before changing any output listed above: an error message, a kernel text face, the
vocabulary's prose or a figure. Keep meaning in words, name the input a message is about, and say
in the pull request what was tested and what was not.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-core/issues/new?labels=accessibility) with the
`accessibility` label (the link sets it when GitHub lets you add labels; otherwise a maintainer
adds it): what you were doing, what happened, and your terminal or browser and any assistive
technology, if you are comfortable sharing them. Anything security-sensitive goes through
[private vulnerability reporting](https://github.com/ikigai-rs/ikigai-core/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

| Date | Change |
|---|---|
| 2026-10-31 | First version. |
