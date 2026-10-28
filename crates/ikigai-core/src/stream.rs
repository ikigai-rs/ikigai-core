//! **The stream builtins** (ledger #2239, stage 3 of the streams design): the generic
//! composers every module reuses instead of writing its own collecting, decoding or range
//! code. The kernel answers them itself, ahead of every chain, as it answers
//! `urn:kernel:*`, so no module can answer for them (they are sealed, [`STREAM_PREFIX`]):
//!
//! | name | what |
//! |---|---|
//! | [`COLLECT`] `urn:stream:collect` | any body (`content`) as one buffer, under the run's byte budget |
//! | [`TEXT`] `urn:stream:text` | a body decoded with its charset (UTF-8 by default) as `text/plain;charset=utf-8`; a malformed body is refused |
//! | [`RANGE`] `urn:stream:range` | the bytes `[start, end)` of the resource `of`, over a seekable body; the answer hangs from `of`'s threads |
//! | [`SPILL`] `urn:stream:spill` | any body (`content`) written through the host's spill facility ([`Spill`](crate::Spill)) and answered as a body that can be read again, under the caller's disk budget |
//!
//! Each declares what it enforces: [`OverBudget`](crate::FailureClass::OverBudget) past the
//! run's byte budget (the grant `urn:cap:stream:memory:<bytes>`, required by none of them:
//! holding none gets the host's base), and none but `spill` requires a capability of its
//! own. `spill` writes to the host's disk, so it requires the disk-budget family
//! `urn:cap:stream:spill:*` ([`CAP_STREAM_SPILL`](crate::CAP_STREAM_SPILL)), and is
//! `OverBudget` past the grant. A body a builtin reads by reference is read under the
//! CALLER's capability, as any sub-request.
//!
//! ```
//! use std::sync::Arc;
//! use futures::executor::block_on;
//! use ikigai_core::{
//!     ArgRef, Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType,
//!     Representation, Request, Verb,
//! };
//!
//! let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
//!     Exact::new("urn:doc"),
//!     FnEndpoint::new("doc", |_| {
//!         Ok(Representation::new(ReprType::new("text/plain"), b"hello, world".to_vec()).cacheable())
//!     }),
//! )));
//! // The kernel answers `urn:stream:*` with no binding of its own.
//! let range = Request::new(Verb::Source, Iri::parse("urn:stream:range").unwrap())
//!     .with_arg("of", ArgRef::Reference(Iri::parse("urn:doc").unwrap()))
//!     .with_arg("start", ArgRef::Inline(b"7".to_vec()))
//!     .with_arg("end", ArgRef::Inline(b"12".to_vec()));
//! let answer = block_on(kernel.issue(range, &Capability::root())).unwrap();
//! assert_eq!(answer.bytes, b"world");
//! ```

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use futures_util::StreamExt;

use crate::body::{Body, ByteStream, Replay};
use crate::content::ContentId;
use crate::describe::{ArgSpec, Description};
use crate::endpoint::{Endpoint, Invocation};
use crate::error::{Error, FailureClass, Result};
use crate::grammar::Exact;
use crate::iri::Iri;
use crate::repr::{ReprType, Representation};
use crate::space::EndpointSpace;
use crate::verb::Verb;

/// The prefix the kernel answers itself for the stream builtins: no space is consulted
/// for a name under it, so no module can answer one.
pub const STREAM_PREFIX: &str = "urn:stream:";

/// `urn:stream:collect`: a body as one buffer under the run's byte budget.
pub const COLLECT: &str = "urn:stream:collect";

/// `urn:stream:text`: a body decoded with its charset, as UTF-8 text.
pub const TEXT: &str = "urn:stream:text";

/// `urn:stream:range`: a byte range of a resource, over a seekable body.
pub const RANGE: &str = "urn:stream:range";

/// `urn:stream:spill`: a body written through the host's spill facility, to be read again.
pub const SPILL: &str = "urn:stream:spill";

const XSD_NON_NEGATIVE_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";
const XSD_ANY_URI: &str = "http://www.w3.org/2001/XMLSchema#anyURI";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const OCTETS: &str = "application/octet-stream";
const TEXT_UTF8: &str = "text/plain;charset=utf-8";

/// The space the kernel answers [`STREAM_PREFIX`] from: the four builtins at their names.
/// Built once per kernel.
pub(crate) fn space() -> EndpointSpace {
    EndpointSpace::new()
        .named(Iri::parse("urn:iki:space:stream").expect("a valid IRI"))
        .bind(Exact::new(COLLECT), Collect)
        .bind(Exact::new(TEXT), Text)
        .bind(Exact::new(RANGE), Range)
        .bind(Exact::new(SPILL), SpillBody)
}

/// Whether the kernel answers `target` from the stream builtins.
pub(crate) fn is_stream(target: &str) -> bool {
    target.starts_with(STREAM_PREFIX)
}

/// The `content` argument as a body with its type: by reference, the referenced answer's
/// own type; inline or by content id, `application/octet-stream`.
async fn content(inv: &Invocation<'_>) -> Result<Representation<Body>> {
    match inv.request.args.get("content") {
        Some(crate::ArgRef::Reference(target)) => {
            let target = target.clone();
            inv.source_body(&target).await
        }
        _ => Ok(Representation::from_body(
            ReprType::new(OCTETS),
            inv.content_body("content").await?,
        )),
    }
}

// ------------------------------------------------------------------------- collect

/// `urn:stream:collect` ([`COLLECT`]).
struct Collect;

#[async_trait]
impl Endpoint for Collect {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = run_budget(inv);
        self.invoke_body(inv).await?.collect_within(&budget).await
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let body = content(inv).await?;
        let repr_type = body.repr_type.clone();
        let bytes = inv.collect(body.bytes).await?;
        Ok(Representation::from_body(repr_type, bytes).cacheable())
    }

    fn name(&self) -> &str {
        "collect"
    }

    fn describe(&self) -> Description {
        Description::new("collect")
            .title("Collect a body")
            .summary(
                "Reads the body given as `content` (inline, by reference, or by content id) into \
                 one buffer, under the run's byte budget: past it the answer is OverBudget, never \
                 a shorter body. The budget is the host's base, raised by a \
                 `urn:cap:stream:memory:<bytes>` grant up to the host's ceiling. A body already \
                 in memory is answered as it is. The answer keeps a referenced body's type.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary("the body to collect")
                    .class(XSD_STRING)
                    .reads_body(),
            )
            .output(OCTETS)
            .fails(FailureClass::OverBudget)
            .fails(FailureClass::Unknown)
    }
}

/// The run's pool, or the default ceiling on a detached invocation.
fn run_budget(inv: &Invocation<'_>) -> crate::MemoryBudget {
    inv.memory_budget()
        .cloned()
        .unwrap_or_else(|| crate::MemoryBudget::new(crate::DEFAULT_STREAM_MEMORY))
}

// ---------------------------------------------------------------------------- text

/// `urn:stream:text` ([`TEXT`]).
struct Text;

/// The charset a `text` request decodes with: the `charset` argument, else the body's own
/// `charset` parameter, else UTF-8. Only UTF-8 is decoded, through std's validator: every
/// other label is refused by name rather than guessed at (WHATWG maps `iso-8859-1` and
/// `us-ascii` to windows-1252, and core carries no such decoder).
fn charset(inv: &Invocation<'_>, body: &Representation<Body>) -> Result<()> {
    let asked =
        match inv.request.args.get("charset") {
            Some(crate::ArgRef::Inline(bytes)) => Some(std::str::from_utf8(bytes).map_err(
                |_| Error::InvalidArgument {
                    name: "charset".into(),
                    detail: "not UTF-8 text".into(),
                },
            )?),
            Some(_) => {
                return Err(Error::InvalidArgument {
                    name: "charset".into(),
                    detail: "expected an inline label such as `utf-8`".into(),
                })
            }
            None => None,
        };
    let label = asked
        .map(str::to_string)
        .or_else(|| body.repr_type.params.get("charset").cloned())
        .unwrap_or_else(|| "utf-8".to_string());
    match label.trim().to_ascii_lowercase().as_str() {
        // The WHATWG Encoding Standard's labels for UTF-8.
        "utf-8" | "utf8" | "unicode-1-1-utf-8" | "unicode11utf8" | "unicode20utf8"
        | "x-unicode20utf8" => Ok(()),
        other => Err(Error::Unsupported {
            verb: None,
            detail: format!("urn:stream:text decodes UTF-8 only; the body's charset is `{other}`"),
        }),
    }
}

/// A malformed body: refused, naming the offset.
fn malformed(at: usize) -> Error {
    Error::InvalidArgument {
        name: "content".into(),
        detail: format!("the body is not valid UTF-8 (at byte {at})"),
    }
}

#[async_trait]
impl Endpoint for Text {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = run_budget(inv);
        self.invoke_body(inv).await?.collect_within(&budget).await
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let body = content(inv).await?;
        charset(inv, &body)?;
        let text = ReprType::new("text/plain").with_param("charset", "utf-8");
        let decoded = match body.bytes {
            // A buffer is validated whole, and is its own UTF-8.
            Body::Bytes(bytes) => match std::str::from_utf8(&bytes) {
                Ok(_) => Body::Bytes(bytes),
                Err(e) => return Err(malformed(e.valid_up_to())),
            },
            // A body that can be read again stays one: each reader validates as it goes.
            Body::Replay(inner) => Body::replay(Utf8Replay(inner)),
            Body::Once(stream) => Body::Once(Box::pin(Utf8Checked::new(stream))),
        };
        Ok(Representation::from_body(text, decoded).cacheable())
    }

    fn name(&self) -> &str {
        "text"
    }

    fn describe(&self) -> Description {
        Description::new("text")
            .title("Decode a body as text")
            .summary(
                "Decodes the body given as `content` with its charset (the `charset` argument, \
                 else the body's own, else UTF-8) and answers `text/plain;charset=utf-8`. \
                 Incremental: a streamed body is validated chunk by chunk, a sequence split \
                 across chunks included, and never collected; a malformed body ends as \
                 InvalidArgument, never as a shorter text. UTF-8 only; another charset is \
                 Unsupported.",
            )
            .transreptor([OCTETS], ["text/plain"])
            .incremental()
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("content")
                    .summary("the body to decode")
                    .class(XSD_STRING)
                    .reads_body(),
            )
            .input(
                ArgSpec::new("charset")
                    .summary("the body's charset when its type does not say; only UTF-8")
                    .class(XSD_STRING)
                    .optional(),
            )
            .input(
                ArgSpec::new("as")
                    .summary("the answer's type")
                    .class(XSD_STRING)
                    .optional(),
            )
            .output(TEXT_UTF8)
            .fails(FailureClass::OverBudget)
            .fails(FailureClass::Unknown)
    }
}

/// A replay whose every reader validates UTF-8 as it reads ([`Text`]).
struct Utf8Replay(Arc<dyn Replay>);

impl Replay for Utf8Replay {
    fn open(&self) -> ByteStream {
        Box::pin(Utf8Checked::new(self.0.open()))
    }

    fn len_hint(&self) -> Option<u64> {
        // Valid UTF-8 is passed through unchanged, so the length is the inner one.
        self.0.len_hint()
    }

    fn validator(&self) -> Option<ContentId> {
        self.0.validator()
    }
}

/// A stream that passes valid UTF-8 through unchanged and ends with
/// [`malformed`] at the first invalid sequence, a sequence split across chunks included:
/// the incomplete tail of one chunk (at most three bytes) is held for the next. A stream
/// that ends inside a sequence is malformed too. Validation is std's
/// (`str::from_utf8`'s `valid_up_to` and `error_len`), never a hand-written decoder.
struct Utf8Checked {
    inner: Option<ByteStream>,
    /// The incomplete sequence the last chunk ended with.
    carry: BytesMut,
    /// How many bytes have been passed on, for the error's offset.
    passed: usize,
}

impl Utf8Checked {
    fn new(inner: ByteStream) -> Self {
        Utf8Checked {
            inner: Some(inner),
            carry: BytesMut::new(),
            passed: 0,
        }
    }

    /// Validate `chunk` after the carried tail: the bytes to pass on (every complete,
    /// valid sequence), keeping a trailing incomplete one; or the offset of an invalid one.
    fn admit(&mut self, chunk: Bytes) -> std::result::Result<Bytes, usize> {
        let joined = if self.carry.is_empty() {
            chunk
        } else {
            self.carry.extend_from_slice(&chunk);
            self.carry.split().freeze()
        };
        match std::str::from_utf8(&joined) {
            Ok(_) => Ok(joined),
            // `error_len` is `None` exactly when the input ENDS inside a sequence that
            // more bytes could complete: hold it for the next chunk.
            Err(e) if e.error_len().is_none() => {
                let valid = e.valid_up_to();
                self.carry.extend_from_slice(&joined[valid..]);
                Ok(joined.slice(..valid))
            }
            Err(e) => Err(self.passed + e.valid_up_to()),
        }
    }
}

impl Stream for Utf8Checked {
    type Item = Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(inner) = self.inner.as_mut() else {
                return Poll::Ready(None);
            };
            match inner.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(error))) => {
                    self.inner = None;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(Some(Ok(chunk))) => match self.admit(chunk) {
                    Ok(valid) if valid.is_empty() => continue,
                    Ok(valid) => {
                        self.passed += valid.len();
                        return Poll::Ready(Some(Ok(valid)));
                    }
                    Err(at) => {
                        self.inner = None;
                        return Poll::Ready(Some(Err(malformed(at))));
                    }
                },
                Poll::Ready(None) => {
                    self.inner = None;
                    if self.carry.is_empty() {
                        return Poll::Ready(None);
                    }
                    let at = self.passed;
                    return Poll::Ready(Some(Err(malformed(at))));
                }
            }
        }
    }
}

// --------------------------------------------------------------------------- range

/// `urn:stream:range` ([`RANGE`]).
struct Range;

/// A non-negative integer argument.
fn offset(inv: &Invocation<'_>, name: &str) -> Result<Option<u64>> {
    if !inv.request.args.contains_key(name) {
        return Ok(None);
    }
    let text = inv.inline_str(name)?;
    text.trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|_| Error::InvalidArgument {
            name: name.into(),
            detail: format!("expected a non-negative integer, got `{text}`"),
        })
}

/// The resource a range is read from: `of`, by reference or as an inline IRI.
fn of(inv: &Invocation<'_>) -> Result<Iri> {
    match inv.request.args.get("of") {
        Some(crate::ArgRef::Reference(iri)) => Ok(iri.clone()),
        Some(crate::ArgRef::Inline(bytes)) => std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| Iri::parse(text.trim()).ok())
            .ok_or_else(|| Error::InvalidArgument {
                name: "of".into(),
                detail: "expected the IRI of the resource to read".into(),
            }),
        Some(crate::ArgRef::Content(_)) => Err(Error::InvalidArgument {
            name: "of".into(),
            detail: "expected the IRI of the resource to read, not content".into(),
        }),
        None => Err(Error::MissingArgument("of".into())),
    }
}

#[async_trait]
impl Endpoint for Range {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = run_budget(inv);
        self.invoke_body(inv).await?.collect_within(&budget).await
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let target = of(inv)?;
        let start = offset(inv, "start")?.ok_or_else(|| Error::MissingArgument("start".into()))?;
        let end = offset(inv, "end")?;
        if let Some(end) = end.filter(|end| *end < start) {
            return Err(Error::InvalidArgument {
                name: "end".into(),
                detail: format!("the range ends before it starts ({start}..{end})"),
            });
        }
        // Read under the caller's capability, recorded as a dependency: the answer hangs
        // from `of`'s threads, so a cut of `of` cuts every cached range of it.
        let source = inv.source_body(&target).await?;
        let repr_type = source.repr_type.clone();
        let unsatisfiable = |len: u64| Error::InvalidArgument {
            name: "start".into(),
            detail: format!(
                "the range starts at {start}, past the end of `{target}` ({len} bytes)"
            ),
        };
        let slice = match source.bytes {
            // A buffer is already in memory: slicing it reads nothing.
            Body::Bytes(bytes) => {
                let len = bytes.len() as u64;
                if start > len || (start == len && len > 0) {
                    return Err(unsatisfiable(len));
                }
                let stop = end.map_or(len, |end| end.min(len));
                Body::Bytes(bytes.slice(start as usize..stop as usize))
            }
            Body::Replay(replay) => {
                if let Some(len) = replay.len_hint() {
                    if start > len || (start == len && len > 0) {
                        return Err(unsatisfiable(len));
                    }
                }
                let Some(stream) = replay.open_at(start) else {
                    return Err(not_seekable(&target));
                };
                let wanted = end.map(|end| end - start);
                let bytes = read_range(stream, wanted, &run_budget(inv)).await?;
                // With no length to check up front, a start past the end shows as nothing
                // to read where something was asked for.
                let asked_some = wanted != Some(0);
                if bytes.is_empty() && start > 0 && asked_some && replay.len_hint().is_none() {
                    return Err(unsatisfiable(start));
                }
                Body::Bytes(bytes)
            }
            Body::Once(_) => return Err(not_seekable(&target)),
        };
        Ok(Representation::from_body(repr_type, slice).cacheable())
    }

    fn name(&self) -> &str {
        "range"
    }

    fn describe(&self) -> Description {
        Description::new("range")
            .title("A byte range of a resource")
            .summary(
                "Reads the bytes from offset `start` up to, not including, offset `end` (the end \
                 of the body when absent; an `end` past it reads to the end) of the resource `of`, \
                 under the caller's capability. The body must be seekable: a buffer, or a \
                 replayable body that opens at an offset. A one-shot or unseekable body is \
                 Unsupported, never collected silently. A `start` past the end is \
                 InvalidArgument (HTTP's 416). The answer keeps the resource's type and hangs \
                 from its threads, so a write to it cuts every cached range. Numeric offsets \
                 only: parsing an HTTP `Range` header is ikigai-http's.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("of")
                    .summary("the resource to read a range of")
                    .class(XSD_ANY_URI),
            )
            .input(
                ArgSpec::new("start")
                    .summary("the first byte's offset")
                    .class(XSD_NON_NEGATIVE_INTEGER),
            )
            .input(
                ArgSpec::new("end")
                    .summary("the offset just past the last byte (exclusive)")
                    .class(XSD_NON_NEGATIVE_INTEGER)
                    .optional(),
            )
            .fails(FailureClass::OverBudget)
            .fails(FailureClass::Unknown)
    }
}

fn not_seekable(target: &Iri) -> Error {
    Error::Unsupported {
        verb: None,
        detail: format!(
            "`{target}` answers a body that cannot be read from an offset (a one-shot stream or \
             an unseekable replay); urn:stream:range does not collect it to slice it"
        ),
    }
}

/// Read at most `wanted` bytes (all, when `None`) of `stream`, drawing them from `budget`,
/// and drop the stream there. A stream that ends sooner is a range clamped to the end.
async fn read_range(
    mut stream: ByteStream,
    wanted: Option<u64>,
    budget: &crate::MemoryBudget,
) -> Result<Bytes> {
    let mut out = BytesMut::new();
    let mut drawn: u64 = 0;
    let result: Result<()> = async {
        while wanted.is_none_or(|w| (out.len() as u64) < w) {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let chunk = chunk?;
            let take = match wanted {
                Some(w) => chunk.len().min((w - out.len() as u64) as usize),
                None => chunk.len(),
            };
            if !budget.try_draw(take as u64) {
                return Err(Error::OverBudget {
                    detail: format!(
                        "the range is larger than the run's byte budget ({} byte(s) left; a \
                         `urn:cap:stream:memory:<bytes>` grant raises it)",
                        budget.remaining()
                    ),
                });
            }
            drawn += take as u64;
            out.extend_from_slice(&chunk[..take]);
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => Ok(out.freeze()),
        Err(error) => {
            budget.give_back(drawn);
            Err(error)
        }
    }
}

// --------------------------------------------------------------------------- spill

/// `urn:stream:spill` ([`SPILL`]).
struct SpillBody;

/// Past the effective spill limit.
fn over_spill(len: u64, limit: u64) -> Error {
    Error::OverBudget {
        detail: format!(
            "the body is {len} bytes, past the spill limit of {limit} (the least of `max-bytes`, \
             the caller's `urn:cap:stream:spill:<bytes>` grant and the host's ceiling)"
        ),
    }
}

#[async_trait]
impl Endpoint for SpillBody {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = run_budget(inv);
        self.invoke_body(inv).await?.collect_within(&budget).await
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<Body>> {
        // The facility and the caller's grant first: with none installed nothing is read.
        let (facility, granted) = inv.spill()?;
        let limit = offset(inv, "max-bytes")?.map_or(granted, |asked| asked.min(granted));
        let source = content(inv).await?;
        let repr_type = source.repr_type.clone();
        // A body that states its length past the limit is refused before a byte is written.
        if let Some(len) = source.bytes.len_hint().filter(|len| *len > limit) {
            return Err(over_spill(len, limit));
        }
        let replay = facility.spill(source.bytes.into_stream(), limit).await?;
        // The facility's contract, checked rather than trusted: a spill is known by its
        // content id and its length, and is never longer than it was allowed to be.
        let (Some(_), Some(len)) = (replay.validator(), replay.len_hint()) else {
            return Err(Error::Endpoint(
                "the spill facility answered a body with no content id or no length; a spill \
                 states both (`Replay::validator`, `Replay::len_hint`)"
                    .into(),
            ));
        };
        if len > limit {
            return Err(over_spill(len, limit));
        }
        // Cacheable: a spill is immutable, and a referenced `content` was read as a
        // recorded dependency, so the answer hangs from its threads.
        Ok(Representation::from_body(repr_type, Body::Replay(replay)).cacheable())
    }

    fn name(&self) -> &str {
        "spill"
    }

    fn describe(&self) -> Description {
        Description::new("spill")
            .title("Spill a body")
            .summary(
                "Writes the body given as `content` (inline, by reference, or by content id) \
                 through the host's spill facility and answers it as a body that can be read \
                 again, known by its content id and length, with a referenced body's type \
                 kept. At most the least of `max-bytes`, the caller's \
                 `urn:cap:stream:spill:<bytes>` grant and the host's ceiling is written: past \
                 it the answer is OverBudget, never a shorter body. A host with no spill \
                 facility (no file system) answers Unsupported. The answer is cacheable and \
                 hangs from a referenced body's threads.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .requires(crate::CAP_STREAM_SPILL)
            .input(
                ArgSpec::new("content")
                    .summary("the body to spill")
                    .class(XSD_STRING)
                    .reads_body(),
            )
            .input(
                ArgSpec::new("max-bytes")
                    .summary("the most to write; lowers the grant, never raises it")
                    .class(XSD_NON_NEGATIVE_INTEGER)
                    .optional(),
            )
            .output(OCTETS)
            .fails(FailureClass::OverBudget)
            // `Unsupported` (no spill facility) is classed Permanent.
            .fails(FailureClass::Permanent)
            .fails(FailureClass::Unknown)
    }
}
