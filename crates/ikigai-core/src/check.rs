//! The one checker: an action's contract applied to a proposed set of named arguments
//! (ledger #1547, design note section 2.1).
//!
//! [`ActionSpec::check`] is the only implementation of the pre-flight rules. Before it,
//! the rules were written several times: `urn:kernel:validate`'s own checker, ikigai-mcp's
//! `validate_arguments` (written from the same rules and already drifted from them), and
//! the Python and Deno faces' `serve`. Every rule a contract gains (a group, a list, a
//! dependency) would have had to be written once per copy. Now `urn:kernel:validate` is a
//! rendering of this function's answer, a Rust host or projection calls it directly, and a
//! face that cannot call Rust is held to it by shared validation vectors.
//!
//! The checker is a PURE function of the contract and the proposed arguments. Authority is
//! not a property of the arguments, so the capability pre-flight `urn:kernel:validate` also
//! runs stays there.

use std::fmt;

use crate::describe::{ActionSpec, ArgSpec, GroupKind, InputGroup, InputSource};
use crate::request::Request;

/// The detail of every refusal of a name given more than once that is not a list
/// argument (ledger #387, #1731): the display form the cli's entry points, ikigai-mcp and
/// [`Request`]'s decoder share, so a typed-error display reads the same from every face.
pub const GIVEN_TWICE: &str = "given twice; an argument takes one value, so name it once";

/// A proposed set of named arguments, in the order they were given, repeats kept.
///
/// Built from the `key=value&key=value` text `urn:kernel:validate` takes
/// ([`parse`](Self::parse)), from pairs (a JSON object, an MCP tool call:
/// [`from_pairs`](Self::from_pairs)), or from a [`Request`] a host is about to issue
/// ([`from_request`](Self::from_request)).
///
/// ```
/// use ikigai_core::ProposedArgs;
///
/// let args = ProposedArgs::parse("start=2026-07-10&mode=add\nflag");
/// let pairs: Vec<_> = args.pairs().collect();
/// assert_eq!(pairs, [("start", "2026-07-10"), ("mode", "add"), ("flag", "")]);
/// assert_eq!(args.get("mode"), Some("add"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProposedArgs {
    pairs: Vec<(String, String)>,
}

impl ProposedArgs {
    /// No arguments.
    pub fn new() -> Self {
        ProposedArgs::default()
    }

    /// Parse `key=value` pairs joined by `&` or newlines, the form `urn:kernel:validate`'s
    /// `args=` takes. Each key and value is trimmed; an empty pair is skipped; a pair with
    /// no `=` is a key with an empty value.
    pub fn parse(text: &str) -> Self {
        let mut pairs = Vec::new();
        for pair in text.split(['&', '\n']) {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            match pair.split_once('=') {
                Some((k, v)) => pairs.push((k.trim().to_string(), v.trim().to_string())),
                None => pairs.push((pair.to_string(), String::new())),
            }
        }
        ProposedArgs { pairs }
    }

    /// The given pairs, in order.
    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        ProposedArgs {
            pairs: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// The arguments a [`Request`] carries: an inline value as its UTF-8 text (lossily), a
    /// reference as its IRI, a content id as its tagged text. A request holds one value
    /// per name, so this never proposes a repeat.
    pub fn from_request(request: &Request) -> Self {
        use crate::arg::ArgRef;
        ProposedArgs {
            pairs: request
                .args
                .iter()
                .map(|(name, arg)| {
                    let value = match arg {
                        ArgRef::Inline(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                        ArgRef::Reference(iri) => iri.as_str().to_string(),
                        ArgRef::Content(id) => id.to_string(),
                    };
                    (name.clone(), value)
                })
                .collect(),
        }
    }

    /// Add one pair (builder).
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.pairs.push((name.into(), value.into()));
        self
    }

    /// Every pair, in the order given.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The first value given for `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether no argument is proposed.
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

/// What rule a [`Violation`] breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ViolationKind {
    /// A required input is not given.
    Missing,
    /// A value is outside the input's `one_of`.
    NotOneOf,
    /// A value does not look like the input's declared XSD datatype.
    Implausible,
    /// A name the action does not declare.
    Unknown,
    /// An [`ExactlyOne`](GroupKind::ExactlyOne) group with none, or more than one, of its
    /// members given.
    ExactlyOne,
    /// An [`AtLeastOne`](GroupKind::AtLeastOne) group with none of its members given.
    AtLeastOne,
    /// An [`AtMostOne`](GroupKind::AtMostOne) group with more than one member given.
    AtMostOne,
    /// A given input whose declared dependency (`needs`) is not given.
    Needs,
    /// A name given more than once that is not a list argument (ledger #387).
    GivenTwice,
}

impl ViolationKind {
    /// The kind's word, as the shared validation vectors spell it.
    pub fn word(self) -> &'static str {
        match self {
            ViolationKind::Missing => "missing",
            ViolationKind::NotOneOf => "not-one-of",
            ViolationKind::Implausible => "implausible",
            ViolationKind::Unknown => "unknown",
            ViolationKind::ExactlyOne => "exactly-one",
            ViolationKind::AtLeastOne => "at-least-one",
            ViolationKind::AtMostOne => "at-most-one",
            ViolationKind::Needs => "needs",
            ViolationKind::GivenTwice => "given-twice",
        }
    }

    /// The SHACL constraint component the rule corresponds to: the
    /// `sh:sourceConstraintComponent` of a validation result reporting it.
    pub fn component(self) -> &'static str {
        match self {
            ViolationKind::Missing => "http://www.w3.org/ns/shacl#MinCountConstraintComponent",
            ViolationKind::NotOneOf => "http://www.w3.org/ns/shacl#InConstraintComponent",
            ViolationKind::Implausible => "http://www.w3.org/ns/shacl#DatatypeConstraintComponent",
            ViolationKind::Unknown => "http://www.w3.org/ns/shacl#ClosedConstraintComponent",
            // `sh:xone` over one `sh:minCount 1` shape per member.
            ViolationKind::ExactlyOne => "http://www.w3.org/ns/shacl#XoneConstraintComponent",
            // `sh:or` of the same.
            ViolationKind::AtLeastOne => "http://www.w3.org/ns/shacl#OrConstraintComponent",
            // A count of the given members, at most one: a SPARQL-based constraint.
            ViolationKind::AtMostOne => "http://www.w3.org/ns/shacl#SPARQLConstraintComponent",
            // `sh:or ( [ sh:path a ; sh:maxCount 0 ] [ sh:path b ; sh:minCount 1 ] )`.
            ViolationKind::Needs => "http://www.w3.org/ns/shacl#OrConstraintComponent",
            // A non-list input is `sh:maxCount 1`; a list drops it.
            ViolationKind::GivenTwice => "http://www.w3.org/ns/shacl#MaxCountConstraintComponent",
        }
    }
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// The contract node a [`Violation`] is about: what a validation report names as its
/// `sh:resultPath`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ViolationPath {
    /// A declared input, by name.
    Input(String),
    /// An [`InputGroup`], by its node segment ([`InputGroup::node_segment`]:
    /// `group:{kind}:{m1},{m2}`).
    Group(String),
}

impl ViolationPath {
    /// This node's IRI under the contract IRI `contract`, spelled as the catalog
    /// (`ikigai-vocab`'s `to_turtle`) spells it: an input is
    /// `{contract}:input:{encode_name(name)}`.
    pub fn node_iri(&self, contract: &str) -> String {
        match self {
            ViolationPath::Input(name) => {
                format!("{contract}:input:{}", crate::encode_name(name))
            }
            ViolationPath::Group(segment) => format!("{contract}:{segment}"),
        }
    }

    /// The path as the shared validation vectors spell it: `input:{name}`, or a group's
    /// node segment.
    pub fn word(&self) -> String {
        match self {
            ViolationPath::Input(name) => format!("input:{name}"),
            ViolationPath::Group(segment) => segment.clone(),
        }
    }
}

/// One rule a proposed call breaks: what [`ActionSpec::check`] returns, and what
/// `urn:kernel:validate` renders as a `sh:ValidationResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Violation {
    /// The rule broken.
    pub kind: ViolationKind,
    /// The contract node it is about, when there is one (an unknown name has none: it is
    /// not in the contract).
    pub path: Option<ViolationPath>,
    /// A human message, quoting the offending name or value.
    pub message: String,
}

impl Violation {
    fn new(kind: ViolationKind, path: Option<ViolationPath>, message: String) -> Self {
        Violation {
            kind,
            path,
            message,
        }
    }

    fn at_input(kind: ViolationKind, input: &ArgSpec, message: String) -> Self {
        Violation::new(
            kind,
            Some(ViolationPath::Input(input.name.clone())),
            message,
        )
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Whether a group member counts as GIVEN: proposed with a value that is not blank. A
/// Sink's engine always sends `content`, empty when there is no body, so an empty value
/// cannot count as choosing that member (ledger #1853: a bare `sink urn:kernel:cut` cut the
/// empty thread).
pub(crate) fn given(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn check_group(group: &InputGroup, proposed: &ProposedArgs) -> Option<Violation> {
    let members: Vec<&str> = group.members.iter().map(String::as_str).collect();
    let present: Vec<&str> = members
        .iter()
        .copied()
        .filter(|m| given(proposed.get(m)))
        .collect();
    let (kind, broken, rule) = match group.kind {
        GroupKind::ExactlyOne => (
            ViolationKind::ExactlyOne,
            present.len() != 1,
            "exactly one of",
        ),
        GroupKind::AtLeastOne => (
            ViolationKind::AtLeastOne,
            present.is_empty(),
            "at least one of",
        ),
        GroupKind::AtMostOne => (
            ViolationKind::AtMostOne,
            present.len() > 1,
            "at most one of",
        ),
    };
    if !broken {
        return None;
    }
    let got = if present.is_empty() {
        "got none".to_string()
    } else {
        format!("got {}", quoted(&present))
    };
    Some(Violation::new(
        kind,
        Some(ViolationPath::Group(group.node_segment())),
        format!("{rule} {} is required; {got}", quoted(&members)),
    ))
}

/// Whether `value` looks like a value of the XSD datatype `class`. Entity classes and
/// datatypes this does not know are not statically checkable, so they always pass.
pub(crate) fn plausible(class: &str, value: &str) -> bool {
    match class {
        "http://www.w3.org/2001/XMLSchema#boolean" => value == "true" || value == "false",
        "http://www.w3.org/2001/XMLSchema#integer" => value.parse::<i64>().is_ok(),
        "http://www.w3.org/2001/XMLSchema#dateTime" => {
            let b = value.as_bytes();
            b.len() >= 10
                && b[..4].iter().all(u8::is_ascii_digit)
                && b[4] == b'-'
                && b[5..7].iter().all(u8::is_ascii_digit)
                && b[7] == b'-'
                && b[8..10].iter().all(u8::is_ascii_digit)
        }
        _ => true,
    }
}

impl ActionSpec {
    /// Check a proposed call against this contract: every rule it breaks, in a stable order
    /// (each input in declaration order, then the names the contract does not declare).
    /// Empty exactly when the arguments conform.
    ///
    /// The rules: a name that is not a [list](ArgSpec::list) is given once (a list's
    /// repeats are joined, as [`join_repeats`](Self::join_repeats) does); a required by-value
    /// input is given; a value (each item, for a list) is one of the input's `one_of`, when
    /// it declares one; a value (each item) is plausible for its declared XSD datatype
    /// (`xsd:boolean`, `xsd:integer`, `xsd:dateTime`; an entity class is not statically
    /// checkable); a given input's `needs` are given; every [`InputGroup`] holds (a member
    /// is given when its value is not blank); every name is declared. A binding is part of
    /// the resource's name, so it is never missing from a proposed call.
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, ProposedArgs, Verb, ViolationKind};
    ///
    /// let sink = ActionSpec::new(Verb::Sink)
    ///     .input(ArgSpec::new("start"))
    ///     .input(ArgSpec::new("mode").one_of(["add", "move"]).optional());
    /// assert!(sink.check(&ProposedArgs::parse("start=today&mode=add")).is_empty());
    ///
    /// let kinds: Vec<_> = sink
    ///     .check(&ProposedArgs::parse("mode=destroy&when=now"))
    ///     .into_iter()
    ///     .map(|v| v.kind)
    ///     .collect();
    /// assert_eq!(
    ///     kinds,
    ///     [ViolationKind::Missing, ViolationKind::NotOneOf, ViolationKind::Unknown]
    /// );
    /// ```
    pub fn check(&self, proposed: &ProposedArgs) -> Vec<Violation> {
        let mut violations = Vec::new();
        let (effective, repeated) = self.joined(proposed);
        for input in &self.inputs {
            if repeated.contains(&input.name.as_str()) {
                violations.push(Violation::at_input(
                    ViolationKind::GivenTwice,
                    input,
                    format!("`{}` is {GIVEN_TWICE}", input.name),
                ));
            }
            let value = effective.get(&input.name);
            if input.required && input.source != InputSource::Binding && value.is_none() {
                violations.push(Violation::at_input(
                    ViolationKind::Missing,
                    input,
                    format!("required input `{}` is missing", input.name),
                ));
                continue;
            }
            let Some(value) = value else { continue };
            match input.list {
                // A list: every item is checked, and each bad one is reported.
                Some(separator) => violations.extend(
                    separator
                        .split(value)
                        .into_iter()
                        .filter_map(|item| check_value(input, item)),
                ),
                None => violations.extend(check_value(input, value)),
            }
            let missing: Vec<&str> = input
                .needs
                .iter()
                .map(String::as_str)
                .filter(|needed| !given(effective.get(needed)))
                .collect();
            if given(Some(value)) && !missing.is_empty() {
                violations.push(Violation::at_input(
                    ViolationKind::Needs,
                    input,
                    format!("`{}` needs {}", input.name, quoted(&missing)),
                ));
            }
        }
        violations.extend(
            self.groups
                .iter()
                .filter_map(|g| check_group(g, &effective)),
        );
        let mut unknown: Vec<&str> = Vec::new();
        for (name, _) in proposed.pairs() {
            if !self.inputs.iter().any(|i| i.name == name) && !unknown.contains(&name) {
                unknown.push(name);
                violations.push(Violation::new(
                    ViolationKind::Unknown,
                    None,
                    format!("unknown argument `{name}` — not in this action's contract"),
                ));
            }
        }
        violations
    }

    /// The proposed arguments with each name given once: the repeats of a LIST argument
    /// joined with its separator's canonical character, so `graph=A graph=B` and
    /// `graph=A,B` are one call (one [`Request::id`], one cache key). A repeat of any other
    /// name is refused as [`InvalidArgument`](crate::Error::InvalidArgument) with the
    /// detail [`GIVEN_TWICE`], the form every cli entry point already uses (ledger #387).
    ///
    /// The one rule every adapter that can see a repeated name (the engine's `k=v k=v`, an
    /// HTTP query string, an MCP array) applies BEFORE it builds a [`Request`], which holds
    /// one value per name.
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, Error, ListSeparator, ProposedArgs, Verb};
    ///
    /// let sparql = ActionSpec::new(Verb::Source)
    ///     .input(ArgSpec::new("graph").optional().list(ListSeparator::CommaOrWhitespace))
    ///     .input(ArgSpec::new("query"));
    /// let joined = sparql
    ///     .join_repeats(&ProposedArgs::parse("graph=urn:a&query=q&graph=urn:b"))
    ///     .unwrap();
    /// assert_eq!(joined.get("graph"), Some("urn:a,urn:b"));
    /// let err = sparql
    ///     .join_repeats(&ProposedArgs::parse("query=a&query=b"))
    ///     .unwrap_err();
    /// assert_eq!(
    ///     err.to_string(),
    ///     "invalid argument `query`: given twice; an argument takes one value, so name it once"
    /// );
    /// assert!(matches!(err, Error::InvalidArgument { .. }));
    /// ```
    pub fn join_repeats(&self, proposed: &ProposedArgs) -> crate::Result<ProposedArgs> {
        let (joined, repeated) = self.joined(proposed);
        match repeated.first() {
            Some(name) => Err(crate::Error::InvalidArgument {
                name: (*name).to_string(),
                detail: GIVEN_TWICE.to_string(),
            }),
            None => Ok(joined),
        }
    }

    /// One value per name, in first-seen order: a list argument's repeats joined, any
    /// other name's FIRST value kept; and the names that were repeated without being a list.
    fn joined<'a>(&self, proposed: &'a ProposedArgs) -> (ProposedArgs, Vec<&'a str>) {
        let mut order: Vec<&str> = Vec::new();
        for (name, _) in proposed.pairs() {
            if !order.contains(&name) {
                order.push(name);
            }
        }
        let mut joined = ProposedArgs::new();
        let mut repeated = Vec::new();
        for name in order {
            let values: Vec<&str> = proposed
                .pairs()
                .filter(|(k, _)| *k == name)
                .map(|(_, v)| v)
                .collect();
            let list = self
                .inputs
                .iter()
                .find(|i| i.name == name)
                .and_then(|i| i.list);
            let value = match (list, values.as_slice()) {
                (_, [one]) => (*one).to_string(),
                (Some(separator), many) => separator.join(many.iter().copied()),
                (None, [first, ..]) => {
                    repeated.push(name);
                    (*first).to_string()
                }
                (None, []) => unreachable!("a name in the order was given"),
            };
            joined = joined.with(name, value);
        }
        (joined, repeated)
    }
}

/// One value (a whole value, or one item of a list) against its input's `one_of` and
/// declared class.
fn check_value(input: &ArgSpec, value: &str) -> Option<Violation> {
    if !input.one_of.is_empty() && !input.one_of.iter().any(|v| v == value) {
        return Some(Violation::at_input(
            ViolationKind::NotOneOf,
            input,
            format!(
                "`{value}` is not an accepted value of `{}` (one of: {})",
                input.name,
                input.one_of.join(", ")
            ),
        ));
    }
    let class = input.class.as_deref()?;
    (!plausible(class, value)).then(|| {
        Violation::at_input(
            ViolationKind::Implausible,
            input,
            format!(
                "`{value}` does not look like a {class} for `{}`",
                input.name
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verb::Verb;

    const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

    fn cal() -> ActionSpec {
        ActionSpec::new(Verb::Sink)
            .input(ArgSpec::new("start").class(XSD_DATETIME))
            .input(ArgSpec::new("mode").one_of(["add", "move"]).optional())
            .input(ArgSpec::new("id").binding())
    }

    #[test]
    fn a_conforming_call_has_no_violation() {
        assert!(cal()
            .check(&ProposedArgs::parse("start=2026-07-10T10:00:00&mode=add"))
            .is_empty());
    }

    #[test]
    fn a_binding_is_never_missing() {
        assert!(cal()
            .check(&ProposedArgs::parse("start=2026-07-10"))
            .is_empty());
    }

    #[test]
    fn each_rule_names_its_input_node() {
        let violations = cal().check(&ProposedArgs::parse("mode=destroy&when=now"));
        let got: Vec<_> = violations
            .iter()
            .map(|v| (v.kind, v.path.as_ref().map(ViolationPath::word)))
            .collect();
        assert_eq!(
            got,
            [
                (ViolationKind::Missing, Some("input:start".to_string())),
                (ViolationKind::NotOneOf, Some("input:mode".to_string())),
                (ViolationKind::Unknown, None),
            ]
        );
        let odd = cal().check(&ProposedArgs::parse("start=whenever"));
        assert_eq!(odd[0].kind, ViolationKind::Implausible);
    }

    #[test]
    fn a_node_iri_encodes_the_input_name_as_the_catalog_does() {
        let path = ViolationPath::Input("100% sure".to_string());
        assert_eq!(path.node_iri("urn:c"), "urn:c:input:100%25%20sure");
    }

    /// Ledger #497, #387: a list's items are checked one by one, its repeats are joined,
    /// and any other repeat is refused, where validate used to keep the first value and the
    /// engine the last.
    #[test]
    fn a_list_checks_each_item_and_joins_its_repeats() {
        use crate::describe::ListSeparator;
        let spec = ActionSpec::new(Verb::Source)
            .input(
                ArgSpec::new("proposals")
                    .optional()
                    .list(ListSeparator::Comma)
                    .one_of(["critical", "major", "minor", "info"]),
            )
            .input(
                ArgSpec::new("graph")
                    .optional()
                    .list(ListSeparator::CommaOrWhitespace)
                    .class("http://www.w3.org/2001/XMLSchema#integer"),
            )
            .input(ArgSpec::new("query").optional());
        let words = |args: &str| -> Vec<(ViolationKind, Option<String>, String)> {
            spec.check(&ProposedArgs::parse(args))
                .into_iter()
                .map(|v| (v.kind, v.path.map(|p| p.word()), v.message))
                .collect()
        };
        let bad = words("proposals=minor,bogus");
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert_eq!(bad[0].0, ViolationKind::NotOneOf);
        assert_eq!(bad[0].1.as_deref(), Some("input:proposals"));
        assert!(bad[0].2.contains("`bogus`") && !bad[0].2.contains("`minor`"));
        assert!(words("graph=1 2,3").is_empty());
        assert_eq!(words("graph=1,x")[0].0, ViolationKind::Implausible);
        // The repeated spelling is the joined one.
        assert!(words("graph=1&graph=2").is_empty());
        assert_eq!(
            spec.join_repeats(&ProposedArgs::parse("graph=1&graph=2"))
                .unwrap()
                .get("graph"),
            Some("1,2")
        );
        // A repeat of a name that is not a list: given twice, once.
        let twice = words("query=a&query=b&query=c");
        assert_eq!(twice.len(), 1, "{twice:?}");
        assert_eq!(twice[0].0, ViolationKind::GivenTwice);
        assert_eq!(twice[0].1.as_deref(), Some("input:query"));
        // An unknown name is reported once, however often it is given.
        assert_eq!(words("x=1&x=2").len(), 1);
    }

    #[test]
    fn the_repeated_and_the_joined_spelling_are_one_request() {
        use crate::arg::ArgRef;
        use crate::describe::ListSeparator;
        use crate::iri::Iri;
        let spec = ActionSpec::new(Verb::Source)
            .input(ArgSpec::new("graph").list(ListSeparator::CommaOrWhitespace));
        let request = |proposed: &ProposedArgs| {
            let joined = spec.join_repeats(proposed).unwrap();
            joined
                .pairs()
                .fold(
                    Request::new(Verb::Source, Iri::parse("urn:q").unwrap()),
                    |r, (k, v)| r.with_arg(k, ArgRef::Inline(v.as_bytes().to_vec())),
                )
                .id()
        };
        assert_eq!(
            request(&ProposedArgs::parse("graph=urn:a&graph=urn:b")),
            request(&ProposedArgs::parse("graph=urn:a,urn:b"))
        );
    }

    #[test]
    fn a_request_proposes_its_arguments() {
        use crate::arg::ArgRef;
        use crate::iri::Iri;
        let request = Request::new(Verb::Sink, Iri::parse("urn:x").unwrap())
            .with_arg("start", ArgRef::Inline(b"2026-07-10".to_vec()))
            .with_arg("who", ArgRef::Reference(Iri::parse("urn:p").unwrap()));
        let proposed = ProposedArgs::from_request(&request);
        assert_eq!(proposed.get("start"), Some("2026-07-10"));
        assert_eq!(proposed.get("who"), Some("urn:p"));
    }
}
