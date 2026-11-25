//! Test-only proof of the proposed optional inline readers; no public API changes.
use ikigai_core::{ArgRef, Bindings, Capability, ContentId, Error, Invocation, Iri, Request, Verb};

fn optional_arg<'a>(inv: &'a Invocation<'_>, name: &str) -> Result<Option<&'a [u8]>, Error> {
    match inv.inline_arg(name) {
        Ok(value) => Ok(Some(value)),
        Err(Error::MissingArgument(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn optional_str<'a>(inv: &'a Invocation<'_>, name: &str) -> Result<Option<&'a str>, Error> {
    match inv.inline_str(name) {
        Ok(value) => Ok(Some(value)),
        Err(Error::MissingArgument(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

#[test]
fn optional_readers_distinguish_absence_from_invalid_values() {
    let bindings = Bindings::new();
    let capability = Capability::root();
    let request = Request::new(Verb::Source, Iri::parse("urn:spike").unwrap())
        .with_arg("empty", ArgRef::Inline(vec![]))
        .with_arg("text", ArgRef::Inline(b"hello".to_vec()))
        .with_arg("invalid", ArgRef::Inline(vec![0xff]))
        .with_arg(
            "reference",
            ArgRef::Reference(Iri::parse("urn:value").unwrap()),
        )
        .with_arg("stored", ArgRef::Content(ContentId::of(b"value")));
    let inv = Invocation::detached(&request, &bindings, &capability);

    assert_eq!(optional_arg(&inv, "absent").unwrap(), None);
    assert_eq!(optional_str(&inv, "absent").unwrap(), None);
    assert_eq!(optional_arg(&inv, "empty").unwrap(), Some(&b""[..]));
    assert_eq!(optional_str(&inv, "text").unwrap(), Some("hello"));
    assert_eq!(optional_arg(&inv, "invalid").unwrap(), Some(&b"\xff"[..]));
    for name in ["invalid", "reference", "stored"] {
        assert!(
            matches!(optional_str(&inv, name), Err(Error::InvalidArgument { name: ref actual, .. }) if actual == name)
        );
    }
    for name in ["reference", "stored"] {
        assert!(
            matches!(optional_arg(&inv, name), Err(Error::InvalidArgument { name: ref actual, .. }) if actual == name)
        );
    }
    // Reproduce the old idiom: a present invalid value silently selects the default.
    assert_eq!(inv.inline_str("invalid").unwrap_or("default"), "default");
    assert_eq!(inv.inline_str("stored").ok(), None);
}
