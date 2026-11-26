# Optional inline arguments

## Problem and proof

An optional argument reader must distinguish an absent argument from a present value it
cannot read. `inv.inline_str(name).ok()` and `inv.inline_str(name).unwrap_or(default)`
discard every error. Invalid UTF-8 and non-inline values therefore select the default just
as a missing argument does. A test-only prototype reproduced that behavior and checked the
proposed distinction. Before the prototype, a test expecting `InvalidArgument` from the
`.ok()` idiom failed for an inline `0xff` value. This is a proof of the reader semantics,
not an audit of every endpoint. The API below shipped in 0.5.1 (and 0.4.3) as
`Invocation::optional_arg` and `optional_str` (ledger #4370);
[`optional_arguments.rs`](../../crates/ikigai-core/tests/optional_arguments.rs), which
replaced the prototype, tests it.

## Proposed core API

Add synchronous methods alongside `inline_arg` and `inline_str`:

```rust
fn optional_arg(&self, name: &str) -> Result<Option<&[u8]>>;
fn optional_str(&self, name: &str) -> Result<Option<&str>>;
```

The returned borrow is tied to the invocation. Only a missing argument becomes `Ok(None)`;
`Ok(Some(value))` includes an empty value. A present value that the corresponding inline
reader cannot use returns `Error::InvalidArgument` naming `name`. The text reader rejects
invalid UTF-8; the byte reader accepts arbitrary bytes. Propagate all other errors rather
than treating them as absence. A caller can use `inv.optional_str("format")?.unwrap_or("text")`
to choose a default only after successful reading. Parsing and validation after reading
must likewise return a named error for an invalid value.

The prototype delegates to the existing readers and translates only `MissingArgument`.
An implementation should preserve that contract if the underlying readers gain new errors.
A declared by-value default is injected before kernel dispatch, so the reader sees it as
present; `None` describes the invocation, not necessarily the original caller's request.

| Argument at the invocation | Proposed behavior |
| --- | --- |
| Absent | `Ok(None)` |
| `Inline` | Borrow the bytes; text additionally requires UTF-8. |
| `Reference` | Refuse with `InvalidArgument` naming the argument; do not resolve it. |
| `Content` (including an attached or stored body) | Refuse with `InvalidArgument` naming the argument; do not fetch or consume the body. |

The kernel may resolve a reference before an endpoint sees it: a declared by-value input
and an argument in an open set receive its representation inline, while an `xsd:anyURI`
input receives the IRI text inline. A `reads_body` input and an undeclared argument in a
closed contract can retain a reference. These readers operate on the resulting invocation,
not the original request. Endpoints that accept bodies should use `content_body`, which
resolves references and attached or stored content asynchronously under the existing
scope and capability rules. A future optional body reader would need its own design: store,
resolution, budget, and one-shot errors must not become absence.

## Migration and prevention

First add and test the core API without changing existing readers. Then make one PR per
repository, reviewing each site rather than mechanically changing it. Replace accidental
error swallowing with the optional reader and `?`; retain deliberate absence-on-error only
with a nearby explanation. Test absent, valid, and present invalid inputs at each affected
endpoint, including non-inline values where they can reach it. Check `.unwrap_or` and other
error-discarding forms as well as `.ok()`; the following baseline counts cover only the
reported non-test `inline_str(...).ok()` pattern on origin/main and must be refreshed at
migration time:

| Repository | Reported sites |
| --- | ---: |
| ikigai-browse | 51 |
| ikigai-ledger | 22 |
| ikigai-repo | 10 |
| ikigai-name | 7 |
| ikigai-personal | 6 |
| ikigai-gonk | 5 |
| ikigai-jsonld | 5 |
| ikigai-engine | 4 |
| ikigai-markdown | 4 |
| ikigai-cli | 3 |
| ikigai-conformance | 3 |
| ikigai-cms-web | 2 |
| ikigai-devtools | 2 |
| ikigai-linkeddata, ikigai-script, ikigai-store, ikigai-ui, ikigai-web, ikigai-xslt | 1 each |

Add a guard after measuring its coverage and false positives. A Clippy disallowed-methods
rule on `Result::ok` would be too broad because it cannot distinguish this receiver; a
source-aware lint could target inline readers, while a `ci-drift` search is easier to deploy
but can miss multiline calls, aliases, and other combinators. Require a documented exception
for deliberate swallowing and include fixtures for accepted and rejected forms. The sweep
and guard should cover both byte and text readers and avoid blocking unrelated uses of
`Result::ok`.

## Proposed follow-up items

- **Add optional inline readers to core** — Implement and document both methods, with tests
  for absent, empty, valid, invalid UTF-8, reference, and content arguments; preserve all
  non-missing errors.
- **Guard against discarded inline-reader errors** — Evaluate and implement a scoped lint or
  `ci-drift` check, with exception handling and fixtures for known bypasses.

- **Migrate ikigai-browse optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-ledger optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-repo optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-name optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-personal optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-gonk optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-jsonld optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-engine optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-markdown optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-cli optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-conformance optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-cms-web optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-devtools optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-linkeddata optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-script optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-store optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-ui optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-web optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
- **Migrate ikigai-xslt optional arguments** — Review each error-swallowing reader, document intentional exceptions, and test invalid values in one PR.
