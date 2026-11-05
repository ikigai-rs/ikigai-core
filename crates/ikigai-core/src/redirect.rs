//! A typed redirect answer: a name that resolves to "elsewhere" (ledger #3150).
//!
//! A redirect is a resolution outcome, not a transport detail, so an endpoint can say it in
//! core's terms and every face can map it. It is a [`Representation`] of the reserved media
//! type [`REDIRECT_MEDIA_TYPE`], not a field: a new field on `Representation` would break
//! every struct literal downstream, and a reserved type crosses a wire as ordinary bytes plus
//! a type, and is cached and golden-threaded like any other answer.

use serde::{Deserialize, Serialize};

use crate::body::Body;
use crate::iri::{Iri, IriError};
use crate::media::media_type_essence;
use crate::repr::{ReprType, Representation};

/// The media type reserved for a [`Redirect`] answer. Its body is the JSON object
/// `{"status":303,"location":"…"}`.
pub const REDIRECT_MEDIA_TYPE: &str = "application/vnd.ikigai.redirect+json";

/// Which redirect an answer is: its HTTP status, and nothing else, since a face that is not
/// HTTP maps it by the same number.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum RedirectStatus {
    /// 301: moved for good.
    MovedPermanently,
    /// 302: elsewhere for now.
    Found,
    /// 303: this name identifies a thing, and that location describes it.
    SeeOther,
    /// 307: elsewhere for now, and the method must not change.
    TemporaryRedirect,
    /// 308: moved for good, and the method must not change.
    PermanentRedirect,
}

impl RedirectStatus {
    /// Every status, for a face that pins its mapping against all of them.
    pub const ALL: [RedirectStatus; 5] = [
        RedirectStatus::MovedPermanently,
        RedirectStatus::Found,
        RedirectStatus::SeeOther,
        RedirectStatus::TemporaryRedirect,
        RedirectStatus::PermanentRedirect,
    ];

    /// The HTTP status code.
    pub fn code(self) -> u16 {
        match self {
            RedirectStatus::MovedPermanently => 301,
            RedirectStatus::Found => 302,
            RedirectStatus::SeeOther => 303,
            RedirectStatus::TemporaryRedirect => 307,
            RedirectStatus::PermanentRedirect => 308,
        }
    }

    /// The status with this code; `None` for any other, 300 and 304 among them, which do not
    /// send a caller to one location.
    pub fn from_code(code: u16) -> Option<RedirectStatus> {
        RedirectStatus::ALL.into_iter().find(|s| s.code() == code)
    }
}

/// A name's answer that the thing is elsewhere: a status and an absolute location.
///
/// An endpoint returns it as `Ok`, through [`Representation::redirect`]; it is an answer, not
/// an [`Error`](crate::Error). It is cacheable and threadable like any representation, and
/// uncacheable until it says otherwise, so a redirect read from a registry hangs from that
/// registry's thread and an edit to the registry cuts it.
///
/// **Core does not follow redirects.** A composite that sources one receives it like any
/// representation, and one that wants to follow it reads
/// [`as_redirect`](Representation::as_redirect) and issues the location itself. A face maps it:
/// HTTP answers the status with a `Location` header (percent-encoding what an IRI allows and a
/// URI does not), and a REPL shows the JSON body, which reads as text.
///
/// **It is for reads.** A `Sink` that answers with this type means nothing special to core,
/// which hands the answer back like any other; a face may refuse it.
///
/// The shape leaves the process, so the bytes are pinned:
///
/// ```
/// use ikigai_core::{Redirect, RedirectStatus, Representation, REDIRECT_MEDIA_TYPE};
///
/// let redirect = Redirect::new(RedirectStatus::SeeOther, "https://example.org/about").unwrap();
/// let answer = Representation::redirect(redirect.clone());
/// assert_eq!(answer.repr_type.media_type, REDIRECT_MEDIA_TYPE);
/// assert_eq!(
///     answer.bytes,
///     br#"{"status":303,"location":"https://example.org/about"}"#
/// );
/// assert_eq!(answer.as_redirect(), Some(redirect));
///
/// let read = answer.as_redirect().unwrap();
/// assert_eq!(read.status.code(), 303);
/// assert_eq!(read.location.as_str(), "https://example.org/about");
///
/// // The location is absolute: a relative one is refused here, not at the face.
/// assert!(Redirect::new(RedirectStatus::Found, "/about").is_err());
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Redirect {
    /// Which redirect it is.
    pub status: RedirectStatus,
    /// Where the thing is. Absolute, and off-site targets are the point.
    pub location: Iri,
}

impl Redirect {
    /// A redirect to `location`, refused unless it is an absolute IRI. An [`Iri`] is accepted
    /// as it is.
    pub fn new(status: RedirectStatus, location: impl Into<String>) -> Result<Self, IriError> {
        Ok(Redirect {
            status,
            location: Iri::parse(location)?,
        })
    }

    /// The redirect in `bytes` under `media_type`: `None` unless the type is the reserved one
    /// and the body is a known status and an absolute location.
    fn read(media_type: &str, bytes: &[u8]) -> Option<Redirect> {
        if media_type_essence(media_type) != REDIRECT_MEDIA_TYPE {
            return None;
        }
        // A derived struct also parses from an array, `[303,"…"]`, which is not this shape.
        if bytes.trim_ascii_start().first() != Some(&b'{') {
            return None;
        }
        let body: RedirectBody = serde_json::from_slice(bytes).ok()?;
        Some(Redirect {
            status: RedirectStatus::from_code(body.status)?,
            location: body.location,
        })
    }
}

/// The JSON a [`Redirect`] crosses in. `Iri`'s deserializer validates, so a relative location
/// fails to parse here as it fails at [`Redirect::new`].
#[derive(Serialize, Deserialize)]
struct RedirectBody {
    status: u16,
    location: Iri,
}

impl Representation<Vec<u8>> {
    /// The answer that says "elsewhere" (uncacheable by default, like
    /// [`new`](Representation::new)). See [`Redirect`].
    pub fn redirect(redirect: Redirect) -> Self {
        let body = RedirectBody {
            status: redirect.status.code(),
            location: redirect.location,
        };
        let bytes = serde_json::to_vec(&body).expect("a number and a string serialize");
        Representation::new(ReprType::new(REDIRECT_MEDIA_TYPE), bytes)
    }

    /// The [`Redirect`] this answer is; `None` for any other type, and for the reserved type
    /// over a body that is not a redirect (malformed JSON, an unknown status, a relative
    /// location), which is never a panic since the bytes may have come from anywhere.
    pub fn as_redirect(&self) -> Option<Redirect> {
        Redirect::read(&self.repr_type.media_type, &self.bytes)
    }
}

impl Representation<Body> {
    /// [`Representation::<Vec<u8>>::as_redirect`] for an answer read through a body entry
    /// point. `None` for a streamed body as well, which this does not consume; a redirect is
    /// a buffer unless its endpoint went out of its way, and such a caller collects it first.
    pub fn as_redirect(&self) -> Option<Redirect> {
        Redirect::read(&self.repr_type.media_type, self.bytes.as_bytes()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reserved(body: &[u8]) -> Representation {
        Representation::new(ReprType::new(REDIRECT_MEDIA_TYPE), body.to_vec())
    }

    #[test]
    fn every_status_round_trips() {
        let codes: Vec<u16> = RedirectStatus::ALL.iter().map(|s| s.code()).collect();
        assert_eq!(codes, [301, 302, 303, 307, 308]);
        for status in RedirectStatus::ALL {
            assert_eq!(RedirectStatus::from_code(status.code()), Some(status));
            let redirect = Redirect::new(status, "https://example.org/x").unwrap();
            let answer = Representation::redirect(redirect.clone());
            assert_eq!(answer.as_redirect(), Some(redirect));
            let expected = format!(
                r#"{{"status":{},"location":"https://example.org/x"}}"#,
                status.code()
            );
            assert_eq!(answer.bytes, expected.as_bytes());
        }
    }

    #[test]
    fn no_other_code_is_a_redirect_status() {
        for code in [0, 200, 300, 304, 305, 306, 309, 410] {
            assert_eq!(RedirectStatus::from_code(code), None, "{code}");
        }
    }

    #[test]
    fn a_non_redirect_representation_reads_as_none() {
        let text = Representation::new(
            ReprType::new("text/plain"),
            br#"{"status":303,"location":"https://example.org/x"}"#.to_vec(),
        );
        assert_eq!(text.as_redirect(), None);
        let json = Representation::new(
            ReprType::new("application/json"),
            br#"{"status":303,"location":"https://example.org/x"}"#.to_vec(),
        );
        assert_eq!(json.as_redirect(), None);
    }

    #[test]
    fn a_malformed_body_under_the_reserved_type_reads_as_none() {
        let bodies: [&[u8]; 14] = [
            b"",
            b"not json",
            b"[303,\"https://example.org/x\"]",
            b"{}",
            b"null",
            b"{\"status\":303}",
            b"{\"location\":\"https://example.org/x\"}",
            b"{\"status\":\"303\",\"location\":\"https://example.org/x\"}",
            b"{\"status\":303.5,\"location\":\"https://example.org/x\"}",
            b"{\"status\":70000,\"location\":\"https://example.org/x\"}",
            b"{\"status\":404,\"location\":\"https://example.org/x\"}",
            b"{\"status\":303,\"location\":7}",
            b"{\"status\":303,\"status\":301,\"location\":\"https://example.org/x\"}",
            b"\xff\xfe",
        ];
        for body in bodies {
            assert_eq!(
                reserved(body).as_redirect(),
                None,
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }

    /// The reader holds a peer's bytes to what the constructor holds an author's to, and a
    /// `Location` header cannot be split by a location that reads at all.
    #[test]
    fn a_location_that_is_not_an_absolute_iri_is_refused_both_ways() {
        for location in [
            "",
            "/about",
            "about",
            "../about",
            "//example.org/about",
            "?q=1",
            "#top",
            "https://example.org/a b",
            "https://example.org/\r\nSet-Cookie: x=y",
        ] {
            assert!(
                Redirect::new(RedirectStatus::Found, location).is_err(),
                "{location:?}"
            );
            let body = serde_json::to_vec(&serde_json::json!({
                "status": 302,
                "location": location,
            }))
            .unwrap();
            assert_eq!(reserved(&body).as_redirect(), None, "{location:?}");
        }
    }

    /// Off-site, non-HTTP and non-ASCII targets all stay exactly as written, and an `Iri` needs
    /// no second spelling.
    #[test]
    fn any_absolute_iri_is_a_location() {
        for location in [
            "https://elsewhere.example/x?y=1#z",
            "urn:name:moved",
            "https://example.org/caf\u{e9}",
        ] {
            let iri = Iri::parse(location).unwrap();
            let redirect = Redirect::new(RedirectStatus::SeeOther, iri).unwrap();
            assert_eq!(redirect.location.as_str(), location);
            let answer = Representation::redirect(redirect.clone());
            assert_eq!(answer.as_redirect(), Some(redirect));
        }
    }

    /// A Python or Deno face writes its own JSON: spaces, the other key order, `\u` escapes.
    /// And a media type compares as a media type, so case and parameters do not hide it.
    #[test]
    fn the_reader_takes_any_json_spelling_and_any_media_type_spelling() {
        let expected = Redirect::new(RedirectStatus::SeeOther, "https://example.org/caf\u{e9}");
        let spaced = reserved(
            br#"{ "location": "https://example.org/caf\u00e9", "status": 303, "note": "extra" }"#,
        );
        assert_eq!(spaced.as_redirect(), expected.clone().ok());
        let spelled = Representation::new(
            ReprType::new("Application/VND.ikigai.Redirect+JSON").with_param("charset", "utf-8"),
            br#"{"status":303,"location":"https://example.org/caf\u00e9"}"#.to_vec(),
        );
        assert_eq!(spelled.as_redirect(), expected.ok());
    }

    #[test]
    fn a_body_answer_reads_when_it_is_a_buffer_and_not_when_it_is_a_stream() {
        let redirect = Redirect::new(RedirectStatus::Found, "https://example.org/x").unwrap();
        let buffered = Representation::redirect(redirect.clone()).into_body();
        assert_eq!(buffered.as_redirect(), Some(redirect));
        let streamed = Representation::from_body(
            ReprType::new(REDIRECT_MEDIA_TYPE),
            Body::once(futures::stream::iter([Ok(bytes::Bytes::from_static(
                br#"{"status":302,"location":"https://example.org/x"}"#,
            ))])),
        );
        assert_eq!(streamed.as_redirect(), None);
    }

    #[test]
    fn a_redirect_is_uncacheable_until_it_says_otherwise() {
        let redirect = Redirect::new(RedirectStatus::Found, "https://example.org/x").unwrap();
        let answer = Representation::redirect(redirect);
        assert_eq!(answer.expiry, crate::repr::Expiry::Always);
        assert_eq!(answer.cacheable().expiry, crate::repr::Expiry::Never);
    }
}
