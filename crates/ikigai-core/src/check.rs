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

use crate::describe::{ActionSpec, ArgSpec, InputSource};
use crate::request::Request;

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
}

impl ViolationKind {
    /// The kind's word, as the shared validation vectors spell it.
    pub fn word(self) -> &'static str {
        match self {
            ViolationKind::Missing => "missing",
            ViolationKind::NotOneOf => "not-one-of",
            ViolationKind::Implausible => "implausible",
            ViolationKind::Unknown => "unknown",
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
        }
    }

    /// The path as the shared validation vectors spell it: `input:{name}`.
    pub fn word(&self) -> String {
        match self {
            ViolationPath::Input(name) => format!("input:{name}"),
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
    /// The rules: a required by-value input is given; a value is one of the input's
    /// `one_of`, when it declares one; a value is plausible for its declared XSD datatype
    /// (`xsd:boolean`, `xsd:integer`, `xsd:dateTime`; an entity class is not statically
    /// checkable); every name is declared. A binding is part of the resource's name, so it
    /// is never missing from a proposed call.
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
        for input in &self.inputs {
            let value = proposed.get(&input.name);
            if input.required && input.source != InputSource::Binding && value.is_none() {
                violations.push(Violation::at_input(
                    ViolationKind::Missing,
                    input,
                    format!("required input `{}` is missing", input.name),
                ));
                continue;
            }
            let Some(value) = value else { continue };
            if !input.one_of.is_empty() && !input.one_of.iter().any(|v| v == value) {
                violations.push(Violation::at_input(
                    ViolationKind::NotOneOf,
                    input,
                    format!(
                        "`{value}` is not an accepted value of `{}` (one of: {})",
                        input.name,
                        input.one_of.join(", ")
                    ),
                ));
                continue;
            }
            if let Some(class) = &input.class {
                if !plausible(class, value) {
                    violations.push(Violation::at_input(
                        ViolationKind::Implausible,
                        input,
                        format!(
                            "`{value}` does not look like a {class} for `{}`",
                            input.name
                        ),
                    ));
                }
            }
        }
        for (name, _) in proposed.pairs() {
            if !self.inputs.iter().any(|i| i.name == name) {
                violations.push(Violation::new(
                    ViolationKind::Unknown,
                    None,
                    format!("unknown argument `{name}` — not in this action's contract"),
                ));
            }
        }
        violations
    }
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
