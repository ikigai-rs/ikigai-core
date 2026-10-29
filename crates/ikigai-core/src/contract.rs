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
//! Both encode their variable segments with [`encode_name`], an **injective**
//! percent-encoding (`%` itself is encoded, unlike
//! [`escape_iri_fragment`](crate::escape_iri_fragment)), so each IRI parses back to exactly
//! what made it, and parses as an IRI whatever the name holds.
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

use std::borrow::Cow;

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

/// The domain tag of the second canonical form (ledger #1547): v1's fields followed by the
/// constructs the ArgSpec release added. Chosen BY CONTENT: a contract that uses none of
/// them is still hashed under v1, so no existing contract IRI moves, and a v1 and a v2
/// encoding can never collide because the tags differ.
const CONTRACT_FORM_V2: &str = "ikigai:contract:v2";

/// A canonical byte encoding with the framing of `crate::hashing` (every variable-length
/// field length-prefixed), for the v2 form's items: each input and each group is encoded
/// whole, and the encodings are sorted, so the set's order is total over every field.
#[derive(Default)]
struct Canon(Vec<u8>);

impl Canon {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn option(&mut self, value: Option<&str>) {
        match value {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                self.str(v);
            }
        }
    }
    fn set<'a>(&mut self, values: impl IntoIterator<Item = &'a str>) {
        let mut sorted: Vec<&str> = values.into_iter().collect();
        sorted.sort();
        sorted.dedup();
        self.u64(sorted.len() as u64);
        for v in sorted {
            self.str(v);
        }
    }
    /// Feed a set of encoded items: sorted, deduplicated, count-prefixed, each
    /// length-prefixed.
    fn feed_items(h: &mut blake3::Hasher, items: impl IntoIterator<Item = Canon>) {
        let mut items: Vec<Vec<u8>> = items.into_iter().map(|c| c.0).collect();
        items.sort();
        items.dedup();
        feed_u64(h, items.len() as u64);
        for item in items {
            crate::hashing::feed_bytes(h, &item);
        }
    }
}

/// Whether an action uses any construct the v1 form cannot carry, so its digest is taken
/// under [`CONTRACT_FORM_V2`].
fn uses_v2(spec: &ActionSpec) -> bool {
    !spec.failures.is_unstated()
        || spec.existence_sensitive
        || !spec.groups.is_empty()
        || !spec.arguments.is_closed()
        || spec.output_from.is_some()
        || spec.inputs.iter().any(|i| {
            !i.needs.is_empty()
                || i.list.is_some()
                || !i.values.is_empty()
                || !i.examples.is_empty()
                || (i.is_enumerated() && i.one_of.is_empty())
                || i.reads_body
                || i.fixed.is_some()
        })
}

/// One input in the v2 form: v1's fields, then the input-level additions.
fn input_v2(input: &ArgSpec) -> Canon {
    let mut c = Canon::default();
    c.str(&input.name);
    c.str(&input.summary);
    c.u8(u8::from(input.required));
    c.u8(match input.source {
        InputSource::Argument => 0,
        InputSource::Binding => 1,
    });
    c.option(input.class.as_deref());
    c.option(input.default.as_deref());
    c.set(input.one_of.iter().map(String::as_str));
    c.set(input.needs.iter().map(String::as_str));
    c.option(input.list.map(|l| l.word()));
    // The value notes, a map sorted by value: each value, its meaning, its tag set and its
    // needs set.
    c.u64(input.values.len() as u64);
    for (value, note) in &input.values {
        c.str(value);
        c.str(&note.summary);
        c.set(note.tags.iter().map(String::as_str));
        c.set(note.needs.iter().map(String::as_str));
    }
    // An EMPTY enumeration (a non-empty one_of already says enumerated), and the examples.
    c.u8(u8::from(input.is_enumerated() && input.one_of.is_empty()));
    c.set(input.examples.iter().map(String::as_str));
    c
}

/// An action's argument set in the v2 form: closed 0, open 1 and its summary, envelope 2
/// and its carrier.
fn arguments_v2(h: &mut blake3::Hasher, arguments: &crate::describe::Arguments) {
    use crate::describe::Arguments;
    match arguments {
        Arguments::Closed => feed_u8(h, 0),
        Arguments::Open(summary) => {
            feed_u8(h, 1);
            feed_str(h, summary);
        }
        Arguments::Envelope(carrier) => {
            feed_u8(h, 2);
            feed_str(h, carrier);
        }
    }
}

/// One input group in the v2 form: its kind, its member set and its pipe member.
fn group_v2(group: &crate::describe::InputGroup) -> Canon {
    let mut c = Canon::default();
    c.str(group.kind.word());
    c.set(group.members.iter().map(String::as_str));
    c.option(group.pipe.as_deref());
    c
}

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

/// Whether a character must be percent-encoded in a NAME: `%` itself (so the encoding is
/// injective), everything a Turtle `IRIREF` cannot carry, `[` and `]` (an IRI admits them
/// only around an IP-literal host, never in the path a name lands in), `#` and `?` (a name
/// is one path segment, and either would end the path there and move the rest of the name
/// into a fragment or a query), and every code point the IRI grammar refuses although an
/// `IRIREF` carries it (DEL, the C1 controls, private use, the noncharacters:
/// [`is_non_ucschar`](crate::iri::is_non_ucschar)).
fn must_encode(c: char) -> bool {
    c == '%'
        || c <= '\u{20}'
        || matches!(
            c,
            '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\' | '[' | ']' | '#' | '?'
        )
        || crate::iri::is_non_ucschar(c)
}

/// Percent-encode a NAME for an IRI position, injectively: the one spelling of every name
/// ikigai mints an IRI from.
///
/// A name is an endpoint id (`urn:ikigai:endpoint:{id}`, and the id in a contract IRI), an
/// input name (the `:input:{name}` nodes, and a validation report's `sh:resultPath`), a
/// door pattern (a match IRI), a thread name, an explain binding key, and a plan's bound,
/// parameter and argument names. `ikigai-vocab`'s `to_turtle` calls this function, so the
/// catalog, the explain graph and the validation report name one node one way (ledger
/// #1345, #1352, #1361).
///
/// Encoded, as the upper-case `%XX` of each UTF-8 byte: `%` (so the encoding is injective,
/// and inverts exactly), what a Turtle `IRIREF` cannot carry (space, the C0 controls,
/// `<>"{}|^` `` ` `` and `\`), `[` and `]`, `#` and `?`, and every code point outside RFC
/// 3987's `ucschar` (DEL, the C1 controls, private use, the noncharacters). Whatever a name
/// holds, the IRI parses, and the whole name stays in the IRI's path. Everything else,
/// ordinary Unicode included, is written as it is, and a name with nothing to encode is
/// returned borrowed.
///
/// ⚠ **`#` and `?` are encoded since 0.1.101, and that RESPELLS names that parsed before**
/// (ledger #1390). A name is one path segment: in `urn:ikigai:endpoint:{id}` a raw `#`
/// moved the rest of the name into a fragment, so anything that drops fragments saw `a#b`
/// and `a#c` as one resource, and a second `#` (id `a#b#c`, or an input `x#y` under id
/// `a#b`) made the document unparseable. A `?` moved the rest into a query the same way.
/// So an id `a#b` was `urn:ikigai:endpoint:a#b` and is now `urn:ikigai:endpoint:a%23b`, and
/// its contract and match IRIs move with it. The brackets and the non-`ucschar` code points
/// (0.1.100, ledger #1361) respelled only names whose IRIs never parsed.
///
/// ⚠ This is for NAMES only. An authored IRI (a `class`, a `requires` scope, a request
/// target) goes through [`escape_iri_fragment`](crate::escape_iri_fragment) instead, which
/// leaves `%`, the brackets, `#` and `?` alone: its escapes are real, an IPv6 host needs
/// its brackets, and an authored IRI may carry a fragment (`xsd:string` is one). A plan id
/// is spelled as a name, keeping only its real `%XX` escapes (`ikigai_vocab::plan`).
///
/// ```
/// use ikigai_core::encode_name;
///
/// assert_eq!(encode_name("to-upper"), "to-upper");
/// assert_eq!(encode_name("naïve"), "naïve");
/// assert_eq!(encode_name("50% off"), "50%25%20off");
/// // An id that is a full IRI with an IPv6 host: the brackets are encoded in a NAME.
/// assert_eq!(encode_name("http://[::1]/x"), "http://%5B::1%5D/x");
/// // DEL, NEL, private use, a noncharacter: no IRI carries them raw.
/// assert_eq!(encode_name("a\u{7F}b\u{85}c"), "a%7Fb%C2%85c");
/// assert_eq!(encode_name("\u{E000}\u{FFFF}"), "%EE%80%80%EF%BF%BF");
/// // A name is one path segment: `#` and `?` would start a fragment or a query.
/// assert_eq!(encode_name("a#b"), "a%23b");
/// assert_eq!(encode_name("a?b"), "a%3Fb");
/// ```
pub fn encode_name(s: &str) -> Cow<'_, str> {
    if !s.chars().any(must_encode) {
        return Cow::Borrowed(s);
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() + 8);
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if must_encode(c) {
            for &byte in c.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Invert [`encode_name`]; `None` for a malformed escape or non-UTF-8 result.
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
        encode_name(pattern)
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
        encode_name(endpoint_id),
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
    /// **Two forms, chosen by content** (ledger #1547). A contract using a construct v1
    /// cannot carry (an input's `needs`, `list`, value notes, examples or EMPTY enumeration,
    /// an [`InputGroup`](crate::InputGroup), an open or envelope argument set, an
    /// `output_from`) is hashed under `ikigai:contract:v2` instead: the same fields, each
    /// input encoded whole with its `needs` set, `list` separator, value notes (sorted by
    /// value: meaning, tag set, needs set), empty-enumeration flag and example set appended
    /// and the inputs sorted by their encodings, then the outputs and scopes as in v1, then
    /// the groups, each as its kind word, member set and pipe member, sorted by their
    /// encodings, then the argument set, then `output_from`. A contract that uses none of them
    /// keeps its v1 digest, so no existing contract IRI moves and a face that predates v2
    /// agrees on every contract it can express.
    ///
    /// **Stated failure classes** (ledger #1546, 0.1.108) put a contract in the v2 form too,
    /// and are appended LAST and only when stated: the byte `1`, then the class
    /// [words](crate::FailureClass::word) as a set of items (sorted by encoding,
    /// deduplicated). So every contract that states none, v1 or v2, keeps its digest:
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, FailureClass, Failures, Verb};
    ///
    /// let quiet = ActionSpec::new(Verb::Source).requires("urn:cap:x");
    /// let mut stated = quiet.clone();
    /// stated.failures = Failures::Only(vec![]);
    /// // Stating even "only the implied ones" is a different contract…
    /// assert_ne!(quiet.contract_id("fetch"), stated.contract_id("fetch"));
    /// // …and the order classes were declared in is not part of it.
    /// let ab = quiet.clone().fails(FailureClass::RetryAfter).fails(FailureClass::Transient);
    /// let ba = quiet.clone().fails(FailureClass::Transient).fails(FailureClass::RetryAfter);
    /// assert_eq!(ab.contract_id("fetch"), ba.contract_id("fetch"));
    /// ```
    ///
    /// **Existence sensitivity** (ledger #2176) puts a contract in the v2 form too, and is
    /// appended after the failure classes, only when set, as the byte `2`. A sensitive
    /// contract is a different contract, and one that is not keeps its digest:
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, Verb};
    ///
    /// let open = ActionSpec::new(Verb::Source).requires("urn:cap:x");
    /// let hidden = open.clone().existence_sensitive();
    /// assert_ne!(open.contract_id("state"), hidden.contract_id("state"));
    /// ```
    ///
    /// **Inputs read as a body** (ledger #2402, [`ArgSpec::reads_body`]) put a contract in
    /// the v2 form too, and are appended after existence sensitivity, only when there are
    /// any, as the byte `3` and the set of their names. A contract that declares none keeps
    /// its digest:
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, Verb};
    ///
    /// let by_value = ActionSpec::new(Verb::Sink).input(ArgSpec::new("content"));
    /// let by_body = ActionSpec::new(Verb::Sink).input(ArgSpec::new("content").reads_body());
    /// assert_ne!(by_value.contract_id("file"), by_body.contract_id("file"));
    /// ```
    ///
    /// **Inputs a named request fixes** (ledger #2512, [`ArgSpec::fixed`]) put a contract in
    /// the v2 form too, and are appended after the body readers, only when there are any,
    /// as the byte `4` and the set of `name`, NUL, `value` strings. A contract that fixes
    /// none keeps its digest:
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, Verb};
    ///
    /// let open = ActionSpec::new(Verb::Source).input(ArgSpec::new("graph").optional());
    /// let bound = ActionSpec::new(Verb::Source).input(ArgSpec::new("graph").fixed_to("urn:g"));
    /// let other = ActionSpec::new(Verb::Source).input(ArgSpec::new("graph").fixed_to("urn:h"));
    /// assert_ne!(open.contract_id("count"), bound.contract_id("count"));
    /// assert_ne!(bound.contract_id("count"), other.contract_id("count"));
    /// ```
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
    /// assert_eq!(a.contract_id("to-upper"), b.contract_id("to-upper"));
    /// assert_ne!(a.contract_id("to-upper"), a.contract_id("to-lower"));
    /// // Tagged, never a bare digest: the text form names its algorithm.
    /// assert!(a.contract_id("to-upper").to_string().starts_with("b3:"));
    /// ```
    pub fn contract_id(&self, endpoint_id: &str) -> ContentId {
        if uses_v2(self) {
            return self.contract_id_v2(endpoint_id);
        }
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

    /// The v2 canonical form (see [`contract_id`](Self::contract_id)).
    fn contract_id_v2(&self, endpoint_id: &str) -> ContentId {
        let mut h = blake3::Hasher::new();
        feed_str(&mut h, CONTRACT_FORM_V2);
        feed_str(&mut h, endpoint_id);
        feed_str(&mut h, &verb_segment(self.verb));
        feed_str(&mut h, &self.summary);
        Canon::feed_items(&mut h, self.inputs.iter().map(input_v2));
        feed_set(&mut h, &self.outputs);
        feed_set(&mut h, &self.requires);
        Canon::feed_items(&mut h, self.groups.iter().map(group_v2));
        arguments_v2(&mut h, &self.arguments);
        match &self.output_from {
            None => feed_u8(&mut h, 0),
            Some(crate::describe::OutputFrom::Input(name)) => {
                feed_u8(&mut h, 1);
                feed_str(&mut h, name);
            }
            Some(crate::describe::OutputFrom::Origin) => feed_u8(&mut h, 2),
        }
        // The failure classes (ledger #1546) are fed ONLY when stated, after everything
        // else: an action that states none keeps the v2 digest it had before 0.1.108. The
        // stream stays unambiguous because every field before this one is self-delimiting,
        // so no unstated stream is a prefix-extension of another contract's stated one.
        if let Some(classes) = self.failures.classes() {
            feed_u8(&mut h, 1);
            Canon::feed_items(
                &mut h,
                classes.iter().map(|class| {
                    let mut c = Canon::default();
                    c.str(class.word());
                    c
                }),
            );
        }
        // Existence sensitivity (ledger #2176) is fed ONLY when set, after the failure
        // classes, as the byte `2`: an action that does not set it keeps its digest, and
        // the byte differs from the failure classes' `1`, so neither tail is the other.
        if self.existence_sensitive {
            feed_u8(&mut h, 2);
        }
        // The inputs read as a body (ledger #2402) are fed ONLY when there are any, after
        // existence sensitivity, as the byte `3` and the set of their names: an action that
        // declares none keeps its digest, and `3` is neither earlier tail's byte. Not inside
        // each input's encoding, which would move every existing v2 contract IRI.
        let readers: Vec<String> = self
            .inputs
            .iter()
            .filter(|i| i.reads_body)
            .map(|i| i.name.clone())
            .collect();
        if !readers.is_empty() {
            feed_u8(&mut h, 3);
            feed_set(&mut h, &readers);
        }
        // The inputs a named request fixes (ledger #2512) are fed ONLY when there are any,
        // after the body readers, as the byte `4` and the set of `name` NUL `value` pairs: an
        // action that fixes none keeps its digest, and `4` is no earlier tail's byte. A NUL
        // cannot occur in an input name, so the pair is unambiguous.
        let fixed: Vec<String> = self
            .inputs
            .iter()
            .filter_map(|i| i.fixed.as_ref().map(|v| format!("{}\0{v}", i.name)))
            .collect();
        if !fixed.is_empty() {
            feed_u8(&mut h, 4);
            feed_set(&mut h, &fixed);
        }
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
        for s in [
            "plain",
            "urn:a:%7B",
            "urn:a:{x}",
            "a b>c",
            "naïve|é",
            "%",
            // ledger #1361: brackets and the non-ucschar code points.
            "http://[::1]/x",
            "]",
            "a\u{7F}b\u{85}c\u{E000}d\u{FDD0}e\u{FFFF}f\u{10FFFF}",
            "%5B[",
            // ledger #1390: a fragment or a query delimiter.
            "a#b",
            "a?b",
            "a#b#c?d?e",
            "%23#%3F?",
        ] {
            assert_eq!(decode_segment(&encode_name(s)).as_deref(), Some(s), "{s}");
        }
        assert_eq!(decode_segment("%G1"), None);
        assert_eq!(decode_segment("%4"), None);
    }

    /// Ledger #1361: encoding the brackets and the non-`ucschar` code points respells NO
    /// name that worked before. Swept over every code point, alone and between letters: a
    /// character outside the new classes encodes exactly as the old rule did (`%` as `%25`,
    /// then the Turtle `IRIREF` set), and a character inside them is one whose old spelling
    /// no IRI parser accepts, so the name it respells produced an unparseable document.
    ///
    /// Ledger #1390 is the deliberate exception: `#` and `?` are the only characters whose
    /// encoding respells a name that parsed (a single one, `a#b`), and the sweep says so
    /// rather than skipping them.
    #[test]
    fn the_new_classes_respell_only_names_that_never_parsed() {
        fn old_rule(s: &str) -> String {
            crate::escape_iri_fragment(&s.replace('%', "%25")).into_owned()
        }
        for cp in 0..=0x10_FFFF {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let name = format!("a{c}b");
            if matches!(c, '#' | '?') {
                // It parsed, with the rest of the name outside the path: respelled anyway.
                let old = format!("urn:ikigai:endpoint:{}", old_rule(&name));
                assert!(oxiri::Iri::parse(old.as_str()).is_ok(), "{old}");
                assert_eq!(encode_name(&name), format!("a%{:02X}b", u32::from(c)));
                let new = format!("urn:ikigai:endpoint:{}", encode_name(&name));
                let parsed = oxiri::Iri::parse(new.as_str()).expect("parses");
                assert_eq!((parsed.fragment(), parsed.query()), (None, None), "{new}");
                continue;
            }
            let new_class = matches!(c, '[' | ']') || crate::iri::is_non_ucschar(c);
            if new_class {
                let old = format!("urn:ikigai:endpoint:{}", old_rule(&name));
                assert!(
                    oxiri::Iri::parse(old.as_str()).is_err(),
                    "U+{cp:04X}: {old}"
                );
                assert_ne!(encode_name(&name), old_rule(&name), "U+{cp:04X}");
            } else {
                assert_eq!(encode_name(&name), old_rule(&name), "U+{cp:04X}");
            }
            let new = format!("urn:ikigai:endpoint:{}", encode_name(&name));
            assert!(oxiri::Iri::parse(new.as_str()).is_ok(), "U+{cp:04X}: {new}");
        }
    }

    #[test]
    fn a_name_with_nothing_to_encode_is_borrowed() {
        let s = "urn:iki:fn:toUpper";
        match encode_name(s) {
            Cow::Borrowed(b) => assert!(std::ptr::eq(b, s)),
            Cow::Owned(o) => panic!("rewritten to {o}"),
        }
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

    /// Ledger #1547: a contract using a construct v1 cannot carry is hashed under v2, every
    /// such construct moves the digest, and a contract using none keeps its v1 digest.
    #[test]
    fn the_v2_form_is_chosen_by_content_and_covers_every_new_construct() {
        use crate::describe::InputGroup;
        let base = ActionSpec::new(Verb::Source)
            .input(ArgSpec::new("a").optional())
            .input(ArgSpec::new("b").optional());
        assert!(!uses_v2(&base));
        let grouped = base.clone().group(InputGroup::exactly_one(["a", "b"]));
        let variants = [
            grouped.clone(),
            base.clone().group(InputGroup::at_least_one(["a", "b"])),
            base.clone()
                .group(InputGroup::exactly_one(["a", "b"]).pipe("a")),
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a").optional().needs(["b"]),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a")
                        .optional()
                        .list(crate::describe::ListSeparator::Comma),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a")
                        .optional()
                        .list(crate::describe::ListSeparator::Newline),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            base.clone()
                .output_from(crate::describe::OutputFrom::Origin),
            base.clone()
                .output_from(crate::describe::OutputFrom::Input("a".to_string())),
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a").optional().example("x"),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a").optional().one_of(Vec::<String>::new()),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            base.clone().open_arguments("others"),
            base.clone().open_arguments("other others"),
            base.clone().envelope("a"),
            base.clone().envelope("b"),
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a")
                        .optional()
                        .one_of(["x", "y"])
                        .value("x", crate::describe::ValueNote::new().tag("t")),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a")
                        .optional()
                        .one_of(["x", "y"])
                        .value("x", crate::describe::ValueNote::new().summary("s")),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
            ActionSpec {
                inputs: vec![
                    ArgSpec::new("a")
                        .optional()
                        .one_of(["x", "y"])
                        .value("x", crate::describe::ValueNote::new().needs(["b"])),
                    ArgSpec::new("b").optional(),
                ],
                ..base.clone()
            },
        ];
        let mut seen = vec![base.contract_id("e")];
        for v in &variants {
            assert!(uses_v2(v), "{v:?}");
            let id = v.contract_id("e");
            assert!(!seen.contains(&id), "{v:?} collides");
            seen.push(id);
        }
        // Members are a set: order is not identity.
        assert_eq!(
            grouped.contract_id("e"),
            base.clone()
                .group(InputGroup::exactly_one(["b", "a", "b"]))
                .contract_id("e")
        );
        // Declaration order of inputs is not identity under v2 either.
        let reordered = ActionSpec::new(Verb::Source)
            .input(ArgSpec::new("b").optional())
            .input(ArgSpec::new("a").optional())
            .group(InputGroup::exactly_one(["a", "b"]));
        assert_eq!(grouped.contract_id("e"), reordered.contract_id("e"));
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
