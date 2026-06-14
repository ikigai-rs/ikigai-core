//! Small closure-backed endpoints — the simplest, idempotent, perfectly
//! cacheable kind. They double as the M1 demonstration of resolution and the
//! pure-cache test: the same call with the same value has the same identity.
//!
//! Per the crate naming conventions, each `snake_case` constructor builds an
//! endpoint whose `lowerCamelCase` identifier matches its name — e.g.
//! [`to_upper`] builds the `toUpper` endpoint resolved at `urn:fn:toUpper`.

use crate::endpoint::{FnEndpoint, Invocation};
use crate::error::{Error, Result};
use crate::repr::{ReprType, Representation};

fn text_plain_utf8() -> ReprType {
    ReprType::new("text/plain").with_param("charset", "utf-8")
}

fn to_upper_impl(inv: &Invocation<'_>) -> Result<Representation> {
    let input = inv.inline_str("in")?;
    Ok(Representation::new(
        text_plain_utf8(),
        input.to_uppercase().into_bytes(),
    ))
}

fn reverse_list_impl(inv: &Invocation<'_>) -> Result<Representation> {
    let input = inv.inline_str("in")?;
    let mut items: Vec<&str> = input.split('\n').collect();
    items.reverse();
    Ok(Representation::new(
        text_plain_utf8(),
        items.join("\n").into_bytes(),
    ))
}

fn echo_impl(inv: &Invocation<'_>) -> Result<Representation> {
    let message = inv
        .bindings
        .get("message")
        .ok_or_else(|| Error::MissingArgument("message".to_string()))?;
    Ok(Representation::new(
        text_plain_utf8(),
        message.as_bytes().to_vec(),
    ))
}

/// `toUpper`: upper-cases the UTF-8 string in the `in` argument.
pub fn to_upper() -> FnEndpoint {
    FnEndpoint::new("toUpper", to_upper_impl)
}

/// `reverseList`: reverses the order of newline-separated items in `in`.
pub fn reverse_list() -> FnEndpoint {
    FnEndpoint::new("reverseList", reverse_list_impl)
}

/// `echo`: returns the `message` variable captured by the resolving grammar
/// (demonstrates grammar bindings flowing to an endpoint).
pub fn echo() -> FnEndpoint {
    FnEndpoint::new("echo", echo_impl)
}
