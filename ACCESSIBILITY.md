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
| Last reviewed | 2026-11-12 |

## What it emits that people read

- **Error messages**: the `Display` of `Error` (`crates/ikigai-core/src/error.rs`), which every
  host prints or serves, for example ``missing required argument `name` ``, ``invalid argument
  `as`: …``, `no endpoint resolved for urn:…`, `denied: …`, `not found: …`, `gone: …`, `gave up after 9 attempts over 120000 ms: …`; and the
  `Display` of the smaller error types (`AliasRefusal` and `AliasParseError` in `alias.rs`,
  `DeclarationError` in `declare.rs`, `TemplateError` in `grammar.rs`, `CacheConfigError`,
  `ContentIdError`, `PrincipalError`, `DynamicRefusal`).
- **Validation messages**: each `Violation` of `urn:kernel:validate` (`src/check.rs`) carries a
  message that names the inputs it is about (for example, which of a group is required, and what
  was given).
- **The kernel's text faces**: the `text/plain` answers of `urn:kernel:aliases`, `cached`,
  `uncached`, `cut`, `threads`, `explain` and the dynamic space's status (`src/kernel_ops.rs`,
  `src/explain.rs`, `src/uncached.rs`, `src/dynamic.rs`): plain lines, some laid out as a
  label column and a value column.
- **Redirect answers**: the JSON body of `Representation::redirect` (`src/redirect.rs`),
  `{"status":303,"location":"…"}`, which a REPL shows as it is.
- **Vocabulary prose**: the `rdfs:label` and `rdfs:comment` text in
  `crates/ikigai-vocab/src/vocabulary.ttl` and `shapes.ttl`, published at
  <https://ikigai-rs.dev/ns> and read by people browsing the vocabulary.
- **Design figures**: three SVG pictures in `docs/design/img/`, shown in
  `docs/design/spaces-as-named-graphs.md`. They are documentation, not output of the crates.
- **A PDF walkthrough**, built on demand by `tools/walkthrough/generate.py` and never committed.

Not this repository's: the pages, terminal screens and colors a host draws around this text, and
the Turtle, HTML or JSON faces other modules render from the kernel's descriptions.

## The rules that output follows

These are contribution rules. The results below distinguish tested behavior from checks
that remain blocked:

- An error names what it is about: a missing or invalid argument by its name, an unresolved
  target and a depth refusal by the IRI, a retry that gave up by its attempts and its time in
  milliseconds (since 2026-11-10). The first words say the kind of failure in words (`denied:`,
  `not found:`, `gone:`, `conflict:`, `timeout:`, `gave up`), never a code alone.
- A kernel operation asked for a face it does not serve refuses with a sentence that names the
  face it was asked for, the operation, and the faces it does serve.
- Keep built-in messages and `text/plain` faces readable without color, terminal escapes or
  rendered markup. JSON and Turtle faces remain structured data; endpoint-supplied text can
  contain arbitrary content.
- What this repository writes uses American spelling.
- Each design figure is `role="img"` with a `<title>` and a `<desc>` that states what the picture
  shows, and every label in it is real `<text>`.

## How it is tested

Every named check has a result: Pass, Fail or Blocked, with evidence. A Pass covers only
its stated scope, not overall accessibility. The checks below were run or reviewed on
2026-11-12 against main commit
[`af918379af14be3361402c9994b753a4843b2adf`](https://github.com/ikigai-rs/ikigai-core/commit/af918379af14be3361402c9994b753a4843b2adf),
before this documentation update. Rust commands used cargo 1.99.1 and rustc 1.99.1;
Python commands used Python 3.13.18. CI repeats the Rust tests and walkthrough anchor
check through `.github/workflows/ci.yml`; the SVG inspection and source reviews are manual
and are not repeated by CI. Once historical, a review that CI does not repeat is Blocked
until rerun, with its original date retained.

| Check | Result | Evidence |
|---|---|---|
| Unsupported-face refusals name the input, requested face, operation and supported faces | Pass | `cargo test --workspace --all-features --test kernel_faces --test american_spelling --test failure_faces --test failure_class_vectors`: `crates/ikigai-core/tests/kernel_faces.rs`, five tests passed, including `single_face_operations_refuse_unsupported_as` and `two_face_operations_refuse_unsupported_as`. Checks the listed kernel operations, not every endpoint's messages. |
| Error display examples | Pass | `cargo test -p ikigai-core --all-features --doc`: 192 doctests passed, including the displayed text for `Error::Gone`, `Conflict`, `RetryAfter`, `OverBudget`, `Refused` and `GaveUp` in `crates/ikigai-core/src/error.rs`. Exact strings, not comprehension or spoken output. |
| Failure-face sentences and shared display vectors | Pass | The combined Rust command above: `crates/ikigai-vocab/tests/failure_faces.rs` (seven passed) checks give-up attempts, elapsed milliseconds and singular `1 attempt`; `failure_class_vectors.rs` (five passed, one regeneration test intentionally ignored) recomputes the display vectors in `tests/vectors/failure_class_vectors.json`. Does not cover every possible error detail. |
| American spelling in the test's scanned files | Pass | The combined Rust command above: `crates/ikigai-core/tests/american_spelling.rs`, two tests passed. A denylist scans Markdown under `docs/` and `crates/`, root `README.md`, Rust under `crates/`, and `vocabulary.ttl`; it excludes marked quotes and its own source. It does not scan this file or `shapes.ttl`, or prove all prose uses American spelling. |
| All other error types, validation messages and kernel text faces: clarity, plain text and no color-only meaning | Blocked | No comprehensive audit of the remaining `Display` implementations, `src/check.rs`, `src/kernel_ops.rs`, `src/explain.rs`, `src/uncached.rs` or `src/dynamic.rs` was run for this update. The selected string tests above do not establish these broader rules. |
| Redirect body explains its numeric status | Fail | Source review of `crates/ikigai-core/src/redirect.rs`: the JSON body contains `status: 303` and `location`, without words explaining 303. No host rendering was evaluated. |
| SVG role, text alternative elements and text labels | Pass | Python `xml.etree.ElementTree` inspection of all three `docs/design/img/*.svg` files asserted `role="img"` and nonempty `title`, `desc` and `text` elements. Source inspection confirms labels are SVG text. Does not verify alternative quality, browser exposure or reading order. |
| SVG explicit title/description association and language | Fail | The same XML inspection found no root `aria-labelledby` or `lang` attribute in any of the three figures; source review also found no `xml:lang`. This is a markup finding, not a browser or WCAG conformance verdict. |
| SVG contrast in light and dark hosts | Blocked | No contrast measurement or rendered light/dark evaluation was run. Source review finds fixed light colors on transparent backgrounds; browser and host backgrounds were not evaluated. |
| Walkthrough annotation anchors | Pass | `python3.13 tools/walkthrough/generate.py --check`: 42 anchors across 17 files resolved. CI runs the same script with Python 3.12. It checks source references only, not PDF accessibility or rendering. |
| Walkthrough generator language, tagging and code size | Fail | Source review of `tools/walkthrough/generate.py`: no document language or tagging is configured; code uses 7.2-point type. The generator does set the document title to `ikigai code walkthrough`. No PDF was built or inspected. |
| Generated PDF accessibility and reading order | Blocked | No PDF was generated for this update; no PDF accessibility checker or reader session was available. The anchor check cannot assess tags, language, contrast, zoom or reading order in an artifact. |
| Vocabulary prose and documentation comprehension | Blocked | No reader evaluation of the documentation, `vocabulary.ttl` or `shapes.ttl` was run; spelling and string assertions are not comprehension checks. |
| Browser rendering, zoom and reflow | Blocked | No browser evaluation of the documentation, figures or a host presenting kernel output was run. This library has no standalone page or terminal interface to evaluate in isolation. |
| Manual keyboard | Blocked | No person performed a keyboard pass in a host presenting this output; no interactive host session was available for this update. |
| Automated accessibility checker | Blocked | No axe, browser accessibility checker or PDF accessibility checker was run; no rendered host or PDF evaluation was configured. |
| Screen reader | Blocked | No screen reader was used on this output; no screen reader session was available for this update. |
| Screen magnifier | Blocked | No screen magnifier was used on this output; no magnifier session was available for this update. |
| Speech input | Blocked | No speech-input software was used on this output; no speech-input session was available for this update. |
| Switch access | Blocked | No switch-access software was used on this output; no switch-access session was available for this update. |
| Braille display | Blocked | No braille display was used on this output; no braille display was available for this update. |

## Known limitations

Source limitations reviewed on 2026-11-12; these are not assistive-technology findings:

- `Error::Endpoint` displays as `endpoint error: …` with whatever text the endpoint gave; it names
  no endpoint or target unless that text does, and most errors say what went wrong but not what
  to do next.
- Messages wrap argument names in backticks (`` `name` ``), Markdown punctuation that a screen
  reader may read aloud in a plain-text face.
- A redirect answer's body states its status as a number alone, with no words for what 303 means.
- The design figures use fixed light colors on a transparent background with no dark-scheme
  variant, so on a dark page the text drawn outside a box may lose contrast; their contrast has
  not been measured. Their root `<svg>` does not reference the `<title>` and `<desc>` with
  `aria-labelledby`, and states no language.
- The walkthrough generator sets a document title, but configures no document language or
  tagging, and sets code in 7.2-point type. The generated artifact has not been evaluated.

## Contributing

Read this file before changing any output listed above: an error message, a kernel text face, the
vocabulary's prose or a figure. Keep meaning in words, name the input a message is about, and say
in the pull request what was tested and what was not. Include a `Check | Result | Evidence`
table in its Accessibility section, using Pass, Fail or Blocked for each check, with the test
or command and tool version, or the reason it could not run. Keep unrun checks as Blocked,
including assistive technologies nobody used. Record a historical check that CI does not
repeat as Blocked with its date until it is rerun.

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
| 2026-11-12 | Record checks as Pass, Fail or Blocked with scope and evidence; rerun text tests and anchor checks on main, inspect SVG structure, and correct the PDF title limitation. |
