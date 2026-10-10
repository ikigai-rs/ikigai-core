//! The two identities an action has (ledger #948): the **match**, one per door and
//! verb, and the **contract** it satisfies, content-addressed.
//!
//! An endpoint's description id is not unique in a kernel: a mount surfaces a peer's
//! endpoint under the peer's id, and one endpoint can sit at two doors. So neither the
//! manifold's rows nor the catalog's contract nodes can be named after the id alone,
//! which is what `urn:ikigai:endpoint:{id}:action:{verb}` did: every copy wrote its
//! triples onto one subject, and a copy with a different contract merged into the
//! other's node.
//!
//! - A **match IRI** names one row of the manifold — a door and a verb:
//!   `urn:ikigai:match:{verb}:{pattern}`. Patterns are distinct per door (resolution
//!   serves the first of any duplicates, and selection keeps only that one), so the
//!   pair is unique. The verb comes first so the pattern, which has colons of its own,
//!   is the whole tail.
//! - A **contract IRI** names one verb's contract by its content:
//!   `urn:ikigai:contract:{id}:{verb}:b3:{hex}`. Identical contracts share a node and
//!   different ones never do. The id and verb are in the IRI for a reader; the digest
//!   ([`ActionSpec::contract_id`]) covers the id, the verb and every triple the
//!   catalog writes on the node, so no two contracts that would render differently can
//!   share one.
//!
//! Both encode their variable segments with an **injective** percent-encoding (`%`
//! itself is encoded, unlike [`escape_iri_fragment`](crate::escape_iri_fragment)), so
//! each IRI parses back to exactly what made it.
//!
//! **Lenient in, canonical out** (Postel's law; ledger #1107). The parse is not
//! canonical-only: [`parse_match_iri`] and [`parse_contract_iri`] accept every spelling
//! that decodes to the same door or contract — lower-case hex (`%7b`), an unnecessary
//! escape (`%41`), a raw character the encoder would have escaped (`{`), an upper-case
//! digest — so a hand-typed IRI, or one that crossed a once-decoding transport (an HTTP
//! query string, a form), still resolves. Everything core EMITS is the canonical
//! spelling: [`match_iri`] and [`ActionSpec::contract_iri`] build it, and an IRI core
//! took from a caller is said back through [`canonical_match_iri`] /
//! [`canonical_contract_iri`] (`urn:kernel:validate`'s report and its errors), never
//! verbatim. So two spellings are two RDF terms on the way in, and one on the way out:
//! a graph, log or trace core writes holds only canonical IRIs, and can be joined on
//! them.
//!
//! The faces that mirror these functions by hand pin themselves against ONE vector file,
//! ikigai-vocab's `tests/vectors/contract_vectors.json`, which a test regenerates from
//! this code and refuses to let drift (ledger #1096).

use crate::content::ContentId;
use crate::describe::{ActionSpec, ArgSpec, InputSource};
use crate::hashing::{feed_str, feed_u64, feed_u8};
use crate::verb::Verb;

/// The prefix of every match IRI: `urn:ikigai:match:{verb}:{pattern}`.
pub const MATCH_PREFIX: &str = "urn:ikigai:match:";

/// The prefix of every content-addressed contract IRI:
/// `urn:ikigai:contract:{id}:{verb}:b3:{hex}`.
pub const CONTRACT_PREFIX: &str = "urn:ikigai:contract:";

/// The domain tag that opens the canonical form. Bumped if the form ever changes, so an
/// old digest can never be mistaken for a new one.
const CONTRACT_FORM: &str = "ikigai:contract:v1";

/// A verb's lower-case name, as the IRIs spell it.
pub(crate) fn verb_segment(verb: Verb) -> String {
    format!("{verb:?}").to_lowercase()
}

fn parse_verb_segment(name: &str) -> Option<Verb> {
    match name {
        "source" => Some(Verb::Source),
        "sink" => Some(Verb::Sink),
        "exists" => Some(Verb::Exists),
        "delete" => Some(Verb::Delete),
        "meta" => Some(Verb::Meta),
        _ => None,
    }
}

/// Whether a character must be percent-encoded in a segment: `%` itself (so the
/// encoding is injective), and everything a Turtle `IRIREF` cannot carry.
fn must_encode(c: char) -> bool {
    c == '%' || c <= '\u{20}' || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\')
}

/// Percent-encode a segment injectively (see the module doc).
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if must_encode(c) {
            for byte in c.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Invert [`encode_segment`]; `None` for a malformed escape or non-UTF-8 result.
///
/// An escape is `%` and exactly two hex digits (RFC 3986 `pct-encoded`). The digits are
/// checked before `u8::from_str_radix`, which also takes a leading `+`: without the
/// check `%+1` decoded to the same byte as `%01` (ledger #1096, the twin of #750 E2).
fn decode_segment(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3)?;
            if !pair.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The match IRI of one door and verb: `urn:ikigai:match:{verb}:{pattern}`, the pattern
/// percent-encoded injectively (a template's braces become `%7B`/`%7D`).
///
/// ```
/// use ikigai_core::{match_iri, parse_match_iri, Verb};
///
/// assert_eq!(
///     match_iri(Verb::Source, "urn:iki:fn:toUpper"),
///     "urn:ikigai:match:source:urn:iki:fn:toUpper"
/// );
/// let template = match_iri(Verb::Source, "urn:file:{path}");
/// assert_eq!(template, "urn:ikigai:match:source:urn:file:%7Bpath%7D");
/// // It parses back to exactly the door and verb that made it.
/// assert_eq!(
///     parse_match_iri(&template),
///     Some((Verb::Source, "urn:file:{path}".to_string()))
/// );
/// // `%` is encoded too, so an escaped pattern and its unescaped twin stay apart.
/// assert_ne!(match_iri(Verb::Source, "urn:a:%7B"), match_iri(Verb::Source, "urn:a:{"));
/// ```
pub fn match_iri(verb: Verb, pattern: &str) -> String {
    format!(
        "{MATCH_PREFIX}{}:{}",
        verb_segment(verb),
        encode_segment(pattern)
    )
}

/// Parse a match IRI back to its verb and door pattern; `None` if `iri` is not one.
pub fn parse_match_iri(iri: &str) -> Option<(Verb, String)> {
    let rest = iri.strip_prefix(MATCH_PREFIX)?;
    let (verb, pattern) = rest.split_once(':')?;
    Some((parse_verb_segment(verb)?, decode_segment(pattern)?))
}

/// The canonical spelling of a match IRI — `match_iri(parse_match_iri(iri))` — or
/// `None` if `iri` is not one. What core says back when a caller hands it any spelling
/// (see the module doc: lenient in, canonical out).
///
/// ```
/// use ikigai_core::canonical_match_iri;
///
/// let canonical = "urn:ikigai:match:source:urn:file:%7Bpath%7D";
/// for spelling in [
///     canonical,
///     "urn:ikigai:match:source:urn:file:%7bpath%7d", // lower-case hex
///     "urn:ikigai:match:source:urn:file:{path}",     // a raw brace
///     "urn:ikigai:match:source:urn:%66ile:%7Bpath%7D", // an unnecessary escape
/// ] {
///     assert_eq!(canonical_match_iri(spelling).as_deref(), Some(canonical));
/// }
/// assert_eq!(canonical_match_iri("urn:ikigai:match:source:%+1"), None);
/// ```
pub fn canonical_match_iri(iri: &str) -> Option<String> {
    parse_match_iri(iri).map(|(verb, pattern)| match_iri(verb, &pattern))
}

/// The canonical spelling of a contract IRI — its parsed id, verb and digest written
/// back as [`ActionSpec::contract_iri`] writes them (id encoded, digest in lower-case
/// hex) — or `None` if `iri` is not one.
///
/// ```
/// use ikigai_core::{canonical_contract_iri, ActionSpec, Verb};
///
/// let canonical = ActionSpec::new(Verb::Source).contract_iri("file");
/// let hex = canonical.rsplit_once(":b3:").unwrap().1;
/// for spelling in [
///     canonical.clone(),
///     format!("urn:ikigai:contract:file:source:b3:{}", hex.to_uppercase()),
///     format!("urn:ikigai:contract:%66ile:source:b3:{hex}"),
/// ] {
///     assert_eq!(canonical_contract_iri(&spelling), Some(canonical.clone()));
/// }
/// ```
pub fn canonical_contract_iri(iri: &str) -> Option<String> {
    parse_contract_iri(iri).map(|(id, verb, digest)| contract_iri_of(&id, verb, &digest))
}

/// A contract IRI from its parts, in the one canonical spelling.
pub(crate) fn contract_iri_of(endpoint_id: &str, verb: Verb, digest: &ContentId) -> String {
    format!(
        "{CONTRACT_PREFIX}{}:{}:{digest}",
        encode_segment(endpoint_id),
        verb_segment(verb)
    )
}

/// Parse a contract IRI into its endpoint id, verb and digest; `None` if `iri` is not
/// one. Parsed from the right, because an id may carry colons of its own.
///
/// ```
/// use ikigai_core::{parse_contract_iri, ActionSpec, Verb};
///
/// let spec = ActionSpec::new(Verb::Source).requires("urn:cap:x");
/// let iri = spec.contract_iri("urn:cms:graph");
/// let (id, verb, digest) = parse_contract_iri(&iri).unwrap();
/// assert_eq!((id.as_str(), verb), ("urn:cms:graph", Verb::Source));
/// assert_eq!(digest, spec.contract_id("urn:cms:graph"));
/// ```
pub fn parse_contract_iri(iri: &str) -> Option<(String, Verb, ContentId)> {
    let rest = iri.strip_prefix(CONTRACT_PREFIX)?;
    let (head, hex) = rest.rsplit_once(":b3:")?;
    let digest = ContentId::parse(&format!("b3:{hex}")).ok()?;
    let (id, verb) = head.rsplit_once(':')?;
    Some((decode_segment(id)?, parse_verb_segment(verb)?, digest))
}

/// One input in canonical order-free form: every field the catalog writes on the
/// input node, with the set-valued `one_of` sorted and deduplicated.
fn canonical_input(input: &ArgSpec) -> ArgSpec {
    let mut one_of = input.one_of.clone();
    one_of.sort();
    one_of.dedup();
    ArgSpec {
        one_of,
        ..input.clone()
    }
}

fn input_key(input: &ArgSpec) -> impl Ord + '_ {
    (
        &input.name,
        &input.summary,
        input.required,
        input.source == InputSource::Binding,
        &input.class,
        &input.default,
        &input.one_of,
    )
}

fn feed_option(h: &mut blake3::Hasher, value: &Option<String>) {
    match value {
        None => feed_u8(h, 0),
        Some(v) => {
            feed_u8(h, 1);
            feed_str(h, v);
        }
    }
}

fn feed_set(h: &mut blake3::Hasher, values: &[String]) {
    let mut sorted: Vec<&String> = values.iter().collect();
    sorted.sort();
    sorted.dedup();
    feed_u64(h, sorted.len() as u64);
    for v in sorted {
        feed_str(h, v);
    }
}

impl ActionSpec {
    /// This verb's contract, content-addressed — the digest a contract IRI carries.
    ///
    /// **The canonical form** (BLAKE3 over the prefix-free encoding of
    /// `crate::hashing`, every variable-length field length-prefixed): the domain tag
    /// `ikigai:contract:v1`; the endpoint id; the verb's lower-case name; the summary;
    /// the inputs, **sorted** (by name, then every other field) and deduplicated, each
    /// as name, summary, required, source, class, default and its `one_of` values
    /// sorted and deduplicated; the outputs, sorted and deduplicated; the required
    /// scopes, sorted and deduplicated. That is every triple the catalog writes on the
    /// contract node and its input nodes, and nothing else — so the order a contract was
    /// DECLARED in never changes its identity, and two contracts that would render
    /// differently never share one.
    ///
    /// The endpoint id is part of the contract: two endpoints with different ids and
    /// identical specs are two tools, and the contract node must say which one it
    /// belongs to.
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, Verb};
    ///
    /// let a = ActionSpec::new(Verb::Source)
    ///     .input(ArgSpec::new("in"))
    ///     .input(ArgSpec::new("locale").optional())
    ///     .requires("urn:cap:a")
    ///     .requires("urn:cap:b");
    /// let b = ActionSpec::new(Verb::Source)
    ///     .requires("urn:cap:b")
    ///     .input(ArgSpec::new("locale").optional())
    ///     .requires("urn:cap:a")
    ///     .input(ArgSpec::new("in"));
    /// assert_eq!(a.contract_id("toUpper"), b.contract_id("toUpper"));
    /// assert_ne!(a.contract_id("toUpper"), a.contract_id("toLower"));
    /// // Tagged, never a bare digest: the text form names its algorithm.
    /// assert!(a.contract_id("toUpper").to_string().starts_with("b3:"));
    /// ```
    pub fn contract_id(&self, endpoint_id: &str) -> ContentId {
        let mut h = blake3::Hasher::new();
        feed_str(&mut h, CONTRACT_FORM);
        feed_str(&mut h, endpoint_id);
        feed_str(&mut h, &verb_segment(self.verb));
        feed_str(&mut h, &self.summary);
        let mut inputs: Vec<ArgSpec> = self.inputs.iter().map(canonical_input).collect();
        inputs.sort_by(|a, b| input_key(a).cmp(&input_key(b)));
        inputs.dedup();
        feed_u64(&mut h, inputs.len() as u64);
        for input in &inputs {
            feed_str(&mut h, &input.name);
            feed_str(&mut h, &input.summary);
            feed_u8(&mut h, u8::from(input.required));
            feed_u8(
                &mut h,
                match input.source {
                    InputSource::Argument => 0,
                    InputSource::Binding => 1,
                },
            );
            feed_option(&mut h, &input.class);
            feed_option(&mut h, &input.default);
            feed_set(&mut h, &input.one_of);
        }
        feed_set(&mut h, &self.outputs);
        feed_set(&mut h, &self.requires);
        ContentId::from_hasher(h)
    }

    /// This verb's contract IRI: `urn:ikigai:contract:{id}:{verb}:b3:{hex}`, the digest
    /// being [`contract_id`](Self::contract_id). The catalog's contract node, and what
    /// [`ActionMatch::action`](crate::ActionMatch::action) carries.
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, Verb};
    ///
    /// let iri = ActionSpec::new(Verb::Sink).contract_iri("cal");
    /// assert!(iri.starts_with("urn:ikigai:contract:cal:sink:b3:"), "{iri}");
    /// assert_eq!(iri.len(), "urn:ikigai:contract:cal:sink:b3:".len() + 64);
    /// // A hostile id cannot close the IRI.
    /// assert!(ActionSpec::new(Verb::Source)
    ///     .contract_iri("ev>il")
    ///     .starts_with("urn:ikigai:contract:ev%3Eil:source:b3:"));
    /// ```
    pub fn contract_iri(&self, endpoint_id: &str) -> String {
        contract_iri_of(endpoint_id, self.verb, &self.contract_id(endpoint_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_round_trip_including_percent_and_non_ascii() {
        for s in ["plain", "urn:a:%7B", "urn:a:{x}", "a b>c", "naïve|é", "%"] {
            assert_eq!(
                decode_segment(&encode_segment(s)).as_deref(),
                Some(s),
                "{s}"
            );
        }
        assert_eq!(decode_segment("%G1"), None);
        assert_eq!(decode_segment("%4"), None);
    }

    /// Ledger #1096: `u8::from_str_radix` takes a leading `+`, so `%+1` decoded to the
    /// same byte as `%01` and two IRIs parsed to one door. A percent escape is `%` and
    /// exactly two hex digits (RFC 3986 `pct-encoded`), as `ContentId::parse` already
    /// insists for the digest (ledger #750, E2).
    #[test]
    fn a_sign_in_a_percent_escape_is_not_hex() {
        assert_eq!(decode_segment("%01").as_deref(), Some("\u{1}"));
        for bad in ["%+1", "%-1", "%+F", "a%+1b", "% 1", "%1 "] {
            assert_eq!(decode_segment(bad), None, "{bad}");
        }
        assert_eq!(parse_match_iri("urn:ikigai:match:source:%+1"), None);
        assert_eq!(
            parse_match_iri("urn:ikigai:match:source:%01"),
            Some((Verb::Source, "\u{1}".to_string()))
        );
        let digest = ActionSpec::new(Verb::Source).contract_id("x");
        let hex = &digest.to_string()["b3:".len()..];
        assert!(parse_contract_iri(&format!("urn:ikigai:contract:%+1:source:b3:{hex}")).is_none());
        assert!(parse_contract_iri(&format!("urn:ikigai:contract:%01:source:b3:{hex}")).is_some());
    }

    #[test]
    fn every_rendered_field_moves_the_digest() {
        let base = ActionSpec::new(Verb::Source)
            .input(ArgSpec::new("in").class("urn:c"))
            .output("text/plain")
            .requires("urn:cap:x");
        let id = base.contract_id("e");
        let variants = [
            base.clone().summary("s"),
            base.clone().input(ArgSpec::new("more").optional()),
            base.clone().output("text/turtle"),
            base.clone().requires("urn:cap:y"),
            ActionSpec {
                verb: Verb::Sink,
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![ArgSpec::new("in")],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![ArgSpec::new("in").class("urn:c").optional()],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![ArgSpec::new("in").class("urn:c").binding()],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![ArgSpec::new("in").class("urn:c").default_value("x")],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![ArgSpec::new("in").class("urn:c").one_of(["a"])],
                ..base.clone()
            },
        ];
        for v in &variants {
            assert_ne!(v.contract_id("e"), id, "{v:?}");
        }
        assert_ne!(base.contract_id("f"), id);
    }

    #[test]
    fn set_valued_fields_are_order_free_and_duplicate_free() {
        let a = ActionSpec::new(Verb::Source)
            .input(ArgSpec::new("m").one_of(["x", "y"]))
            .output("a/b")
            .output("c/d");
        let b = ActionSpec::new(Verb::Source)
            .input(ArgSpec::new("m").one_of(["y", "x", "y"]))
            .output("c/d")
            .output("a/b")
            .output("a/b");
        assert_eq!(a.contract_id("e"), b.contract_id("e"));
    }

    #[test]
    fn a_contract_iri_parses_back() {
        let spec = ActionSpec::new(Verb::Delete);
        for id in ["toUpper", "urn:cms:graph", "ev>il", "a%3Eb"] {
            let (got, verb, digest) = parse_contract_iri(&spec.contract_iri(id)).unwrap();
            assert_eq!((got.as_str(), verb), (id, Verb::Delete));
            assert_eq!(digest, spec.contract_id(id));
        }
        assert!(parse_contract_iri("urn:ikigai:endpoint:x:action:source").is_none());
        assert!(parse_match_iri("urn:ikigai:match:frobnicate:urn:x").is_none());
    }
}
