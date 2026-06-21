//! Small closure-backed endpoints — the simplest, idempotent, perfectly
//! cacheable kind. They double as the M1 demonstration of resolution and the
//! pure-cache test: the same call with the same value has the same identity.
//!
//! Per the crate naming conventions, each `snake_case` constructor builds an
//! endpoint whose `lowerCamelCase` identifier matches its name — e.g.
//! [`to_upper`] builds the `toUpper` endpoint resolved at `urn:fn:toUpper`.
//!
//! Recursive `$a{<iri>}` transclusion (`compose`) lives in the `ikigai-fn` module
//! crate, not here — it is a host-facing function, and the kernel only needs the
//! [`fan_out`](crate::Invocation::fan_out) seam it builds on.

use crate::describe::{ArgSpec, Description};
use crate::endpoint::{FnEndpoint, Invocation};
use crate::error::{Error, Result};
use crate::repr::{ReprType, Representation};
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
