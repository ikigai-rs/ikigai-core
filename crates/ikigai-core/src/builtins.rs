//! Small closure-backed endpoints — the simplest, idempotent, perfectly
//! cacheable kind. They double as the M1 demonstration of resolution and the
//! pure-cache test: the same call with the same value has the same identity.
//!
//! Per the crate naming conventions, each `snake_case` constructor builds an
//! endpoint whose `lowerCamelCase` identifier matches its name — e.g.
//! [`to_upper`] builds the `toUpper` endpoint resolved at `urn:fn:toUpper`.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use futures_util::future::try_join_all;

use crate::arg::ArgRef;
use crate::describe::{ArgSpec, Description};
use crate::endpoint::{Endpoint, FnEndpoint, Invocation};
use crate::error::{Error, Result};
use crate::iri::Iri;
use crate::repr::{ReprType, Representation};
use crate::request::Request;
use crate::verb::Verb;

fn text_plain_utf8() -> ReprType {
    ReprType::new("text/plain").with_param("charset", "utf-8")
}

fn to_upper_impl(inv: &Invocation<'_>) -> Result<Representation> {
    let input = inv.inline_str("in")?;
    Ok(Representation::new(text_plain_utf8(), input.to_uppercase().into_bytes()).cacheable())
}

fn reverse_list_impl(inv: &Invocation<'_>) -> Result<Representation> {
    let input = inv.inline_str("in")?;
    let mut items: Vec<&str> = input.split('\n').collect();
    items.reverse();
    Ok(Representation::new(text_plain_utf8(), items.join("\n").into_bytes()).cacheable())
}

fn echo_impl(inv: &Invocation<'_>) -> Result<Representation> {
    let message = inv
        .bindings
        .get("message")
        .ok_or_else(|| Error::MissingArgument("message".to_string()))?;
    Ok(Representation::new(text_plain_utf8(), message.as_bytes().to_vec()).cacheable())
}

const TEXT_PLAIN_UTF8: &str = "text/plain;charset=utf-8";

/// `toUpper`: upper-cases the UTF-8 string in the `in` argument.
pub fn to_upper() -> FnEndpoint {
    FnEndpoint::new("toUpper", to_upper_impl).with_description(
        Description::new("toUpper")
            .title("Upper-case")
            .summary("Upper-cases the UTF-8 text supplied in the `in` argument.")
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(ArgSpec::new("in").summary("the text to upper-case"))
            .output(TEXT_PLAIN_UTF8),
    )
}

/// `reverseList`: reverses the order of newline-separated items in `in`.
pub fn reverse_list() -> FnEndpoint {
    FnEndpoint::new("reverseList", reverse_list_impl).with_description(
        Description::new("reverseList")
            .title("Reverse list")
            .summary("Reverses the order of newline-separated items in the `in` argument.")
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(ArgSpec::new("in").summary("newline-separated items"))
            .output(TEXT_PLAIN_UTF8),
    )
}

/// `echo`: returns the `message` variable captured by the resolving grammar
/// (demonstrates grammar bindings flowing to an endpoint).
pub fn echo() -> FnEndpoint {
    FnEndpoint::new("echo", echo_impl).with_description(
        Description::new("echo")
            .title("Echo")
            .summary("Returns the `message` segment captured from the resource identifier.")
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("message")
                    .summary("the text to echo, captured from the path by the resolving grammar")
                    .binding(),
            )
            .output(TEXT_PLAIN_UTF8),
    )
}

/// Maximum `$a{}` expansion depth — a backstop against a shape that transcludes
/// itself, directly or through a cycle.
const COMPOSE_MAX_DEPTH: usize = 32;

/// `compose`: recursive resource transclusion.
///
/// Sources the resource named by the `src` argument and expands every
/// `$a{<iri>}` marker in its (UTF-8 text) representation by resolving the
/// embedded resource through the kernel and splicing the result in — recursively,
/// so a transcluded shape may itself contain markers. A marker may carry inline
/// arguments (`$a{urn:fn:toUpper?in="resource oriented computing"}`); a literal
/// marker is written `$$a{…}` (a `$$` is a literal `$`).
///
/// The `a` is for *asynchronous*: the markers at one level are forked and joined,
/// so a kernel driven on a concurrent executor resolves them simultaneously,
/// while a single-threaded executor (the browser, for now) resolves them in turn.
///
/// The output mirrors the source's media type. It declares itself cacheable, so
/// the kernel keeps it cacheable only while every transcluded part is — one
/// volatile constituent makes the whole composite volatile, automatically.
pub struct Compose;

#[async_trait]
impl Endpoint for Compose {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let src = inv.inline_str("src")?;
        let iri = Iri::parse(src).map_err(|e| Error::InvalidArgument {
            name: "src".to_string(),
            detail: format!("not an IRI: {e}"),
        })?;
        let shape = inv.source(&iri).await?;
        let Representation {
            repr_type, bytes, ..
        } = shape;
        let text = String::from_utf8(bytes).map_err(|_| {
            Error::Endpoint(format!("compose: `{}` is not UTF-8 text", iri.as_str()))
        })?;
        let expanded = expand(inv, text, 0).await?;
        Ok(Representation::new(repr_type, expanded.into_bytes()).cacheable())
    }

    fn name(&self) -> &str {
        "compose"
    }

    fn describe(&self) -> Description {
        Description::new("compose")
            .title("Compose")
            .summary(
                "Recursively expands `$a{<iri>}` transclusion markers in the resource named by \
                 the `src` argument, resolving each embedded resource through the kernel and \
                 splicing it in. A literal marker is written `$$a{…}`. The output mirrors the \
                 source's media type and stays cacheable only while every transcluded part is.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(ArgSpec::new("src").summary("the IRI of the shape resource to compose"))
            .output("text/html")
    }
}

/// `compose`: recursive `$a{<iri>}` resource transclusion. See [`Compose`].
pub fn compose() -> Compose {
    Compose
}

/// One piece of a scanned shape: literal text, or a marker body to resolve.
enum Segment {
    Lit(String),
    Marker(String),
}

/// Split `text` into ordered literal/marker segments. `$$` collapses to a literal
/// `$` (so `$$a{…}` becomes the literal text `$a{…}`); `$a{ … }` becomes a marker
/// holding its inner `<iri>[?args]`; an unterminated `$a{` stays literal.
fn scan(text: &str) -> Vec<Segment> {
    let b = text.as_bytes();
    let mut segments = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            // `$$` is a literal `$` — collapses the escape for `$a{`.
            if b.get(i + 1) == Some(&b'$') {
                lit.push('$');
                i += 2;
                continue;
            }
            // `$a{ … }` — a transclusion marker.
            if text[i..].starts_with("$a{") {
                let brace = i + 2; // the `{`
                if let Some(close) = find_marker_end(text, brace) {
                    if !lit.is_empty() {
                        segments.push(Segment::Lit(std::mem::take(&mut lit)));
                    }
                    segments.push(Segment::Marker(text[brace + 1..close].trim().to_string()));
                    i = close + 1;
                    continue;
                }
                // Unterminated marker: fall through and keep the `$` literal.
            }
        }
        let len = utf8_len(b[i]);
        lit.push_str(&text[i..i + len]);
        i += len;
    }
    if !lit.is_empty() {
        segments.push(Segment::Lit(lit));
    }
    segments
}

/// Expand every `$a{<iri>}` marker in `text`. The `a` is for *asynchronous*: a
/// level's markers are forked and joined, so a concurrency-capable kernel pulls
/// them simultaneously, while a single-threaded executor (the browser, for now)
/// resolves them in turn. Each result is itself expanded, so a transcluded
/// shape's own markers recurse. Boxed for the async recursion.
fn expand<'a>(
    inv: &'a Invocation<'_>,
    text: String,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
    Box::pin(async move {
        if depth >= COMPOSE_MAX_DEPTH {
            return Err(Error::Endpoint(format!(
                "compose: recursion limit ({COMPOSE_MAX_DEPTH}) exceeded — cyclic transclusion?"
            )));
        }
        let segments = scan(&text);
        // Fork: one resolution future per marker, in document order.
        let jobs: Vec<_> = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Marker(inner) => Some(resolve_marker(inv, inner.clone(), depth)),
                Segment::Lit(_) => None,
            })
            .collect();
        // Join: resolve the level concurrently (the kernel parallelizes if it can).
        let mut resolved = try_join_all(jobs).await?.into_iter();
        // Reassemble in document order.
        let mut out = String::with_capacity(text.len());
        for segment in &segments {
            match segment {
                Segment::Lit(t) => out.push_str(t),
                Segment::Marker(_) => {
                    out.push_str(&resolved.next().expect("one result per marker"))
                }
            }
        }
        Ok(out)
    })
}

/// Resolve one marker body `<iri>[?args]` through the kernel, then expand the
/// result so a transcluded shape's own markers recurse. Non-text isn't inlined.
fn resolve_marker<'a>(
    inv: &'a Invocation<'_>,
    inner: String,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
    Box::pin(async move {
        let repr = inv.issue(parse_marker(&inner)?).await?;
        match String::from_utf8(repr.bytes) {
            Ok(s) => expand(inv, s, depth + 1).await,
            Err(e) => Ok(format!(
                "<!-- compose: `{inner}` is non-text ({} bytes), not inlined -->",
                e.into_bytes().len()
            )),
        }
    })
}

/// The index of the `}` closing the marker whose `{` is at `brace`, skipping any
/// `}` inside a `"…"` span (where `\"` and `\\` are escapes). `None` if unterminated.
fn find_marker_end(text: &str, brace: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = brace + 1;
    let mut in_quote = false;
    while i < b.len() {
        match b[i] {
            b'\\' if in_quote => i += 2,
            b'"' => {
                in_quote = !in_quote;
                i += 1;
            }
            b'}' if !in_quote => return Some(i),
            c => i += utf8_len(c),
        }
    }
    None
}

/// Parse a marker body `<iri>[?k=v&…]` into a SOURCE request.
fn parse_marker(inner: &str) -> Result<Request> {
    let (iri_str, query) = match inner.split_once('?') {
        Some((iri, q)) => (iri.trim(), Some(q)),
        None => (inner, None),
    };
    let iri = Iri::parse(iri_str)
        .map_err(|e| Error::Endpoint(format!("compose: bad IRI in marker `{inner}`: {e}")))?;
    let mut request = Request::new(Verb::Source, iri);
    if let Some(q) = query {
        for (key, value) in parse_query(q)? {
            request = request.with_arg(key, ArgRef::Inline(value.into_bytes()));
        }
    }
    Ok(request)
}

/// Parse `k=v&k2="v with spaces"` marker arguments. A value may be double-quoted
/// (the quotes are stripped and `\"` / `\\` unescaped inside).
fn parse_query(query: &str) -> Result<Vec<(String, String)>> {
    let mut args = Vec::new();
    for pair in query.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            Error::Endpoint(format!(
                "compose: marker argument `{pair}` is not key=value"
            ))
        })?;
        args.push((key.trim().to_string(), unquote(value.trim())));
    }
    Ok(args)
}

/// Strip surrounding double quotes from a marker argument value, unescaping
/// `\"` and `\\`. An unquoted value is returned unchanged.
fn unquote(value: &str) -> String {
    let b = value.as_bytes();
    if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
        let mut out = String::with_capacity(value.len() - 2);
        let mut chars = value[1..value.len() - 1].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next().unwrap_or('\\')),
                _ => out.push(c),
            }
        }
        out
    } else {
        value.to_string()
    }
}

/// The byte length of the UTF-8 sequence starting with `first`.
fn utf8_len(first: u8) -> usize {
    match first {
        b if b < 0x80 => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod compose_tests {
    use super::*;
    use crate::capability::Capability;
    use crate::grammar::Exact;
    use crate::kernel::Kernel;
    use crate::repr::Expiry;
    use crate::space::EndpointSpace;
    use futures::executor::block_on;
    use std::sync::Arc;

    /// A shape resource: returns a fixed `text/html` body (which may carry markers).
    fn shape(html: &'static str) -> FnEndpoint {
        FnEndpoint::new("shape", move |_inv: &Invocation<'_>| {
            Ok(
                Representation::new(ReprType::new("text/html"), html.as_bytes().to_vec())
                    .cacheable(),
            )
        })
    }

    /// A kernel binding `compose`, `toUpper`, and a `urn:data:page` shape.
    fn kernel(page: &'static str) -> Kernel {
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:fn:compose"), compose())
            .bind(Exact::new("urn:fn:toUpper"), to_upper())
            .bind(Exact::new("urn:data:page"), shape(page));
        Kernel::new(Arc::new(space))
    }

    fn compose_page(kernel: &Kernel) -> Representation {
        block_on(
            kernel.issue(
                Request::new(Verb::Source, Iri::parse("urn:fn:compose").unwrap())
                    .with_arg("src", ArgRef::Inline(b"urn:data:page".to_vec())),
                &Capability::root(),
            ),
        )
        .unwrap()
    }

    #[test]
    fn expands_a_marker_with_a_quoted_argument() {
        let rep = compose_page(&kernel(r#"<h1>$a{urn:fn:toUpper?in="hi there"}</h1>"#));
        assert_eq!(rep.bytes, b"<h1>HI THERE</h1>".to_vec());
    }

    #[test]
    fn preserves_the_source_media_type() {
        let rep = compose_page(&kernel("<p>$a{urn:fn:toUpper?in=x}</p>"));
        assert_eq!(rep.repr_type.media_type, "text/html");
    }

    #[test]
    fn a_double_dollar_keeps_a_marker_literal() {
        let rep = compose_page(&kernel("show $$a{urn:fn:toUpper?in=x} verbatim"));
        assert_eq!(rep.bytes, b"show $a{urn:fn:toUpper?in=x} verbatim".to_vec());
    }

    #[test]
    fn recurses_into_transcluded_shapes() {
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:fn:compose"), compose())
            .bind(Exact::new("urn:fn:toUpper"), to_upper())
            .bind(Exact::new("urn:data:page"), shape("[$a{urn:data:inner}]"))
            .bind(
                Exact::new("urn:data:inner"),
                shape("$a{urn:fn:toUpper?in=hi}"),
            );
        let kernel = Kernel::new(Arc::new(space));
        let rep = compose_page(&kernel);
        assert_eq!(rep.bytes, b"[HI]".to_vec());
    }

    #[test]
    fn a_composite_of_cacheable_parts_is_cacheable() {
        let rep = compose_page(&kernel("<p>$a{urn:fn:toUpper?in=hi}</p>"));
        assert_eq!(rep.expiry, Expiry::Never);
    }

    #[test]
    fn text_around_and_between_markers_is_preserved() {
        let rep = compose_page(&kernel(
            "a $a{urn:fn:toUpper?in=b} c $a{urn:fn:toUpper?in=d} e",
        ));
        assert_eq!(rep.bytes, b"a B c D e".to_vec());
    }
}
