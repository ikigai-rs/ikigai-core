use serde::{Deserialize, Serialize};

use crate::verb::Verb;

/// Where an input's value comes from — the two channels an endpoint can read a
/// parameter through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSource {
    /// A by-value argument supplied alongside the request and content-addressed
    /// into its identity (`ArgRef::Inline` / `Content`). The default.
    #[default]
    Argument,
    /// A variable captured from the resource identifier by the resolving grammar
    /// (e.g. a `{var}` in a [`UriTemplate`](crate::UriTemplate)). Its value lives
    /// in the IRI, so it is part of the resource's identity directly.
    Binding,
}

/// A specification of one named input an endpoint accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgSpec {
    /// The argument name.
    pub name: String,
    /// A short human description.
    pub summary: String,
    /// Whether the argument is required.
    pub required: bool,
    /// Whether the value arrives as a by-value argument or a grammar binding.
    #[serde(default)]
    pub source: InputSource,
    /// The RDF class this input's value is expected to be, as an IRI (e.g.
    /// `https://schema.org/Person`). Optional; when present it lets selection match an
    /// endpoint to the *types* available in a context — "what can I do with these entities?"
    /// (`select_action`) — the same way `transreptsFrom`/`To` drives transreptor selection.
    /// Omitted from serialized output when absent, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

impl ArgSpec {
    /// A required by-value argument with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        ArgSpec {
            name: name.into(),
            summary: String::new(),
            required: true,
            source: InputSource::Argument,
            class: None,
        }
    }

    /// Add a human summary (builder).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Declare the RDF class (IRI) this input's value is expected to be (builder) — e.g.
    /// `https://schema.org/Person`. Drives type-based selection (`select_action`).
    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.class = Some(class.into());
        self
    }

    /// Mark the argument optional (builder).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Declare that this input is captured from the resource identifier by the
    /// resolving grammar rather than passed as a by-value argument (builder).
    pub fn binding(mut self) -> Self {
        self.source = InputSource::Binding;
        self
    }
}

/// A structured, RDF-agnostic self-description of an endpoint.
///
/// `ikigai-core` keeps this free of any RDF dependency; `ikigai-vocab` projects
/// it to RDF (e.g. Turtle) so a `Meta` request can return a machine-readable
/// description of an endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Description {
    /// The endpoint identifier (a `lowerCamelCase` resource identifier).
    pub id: String,
    /// A short human title.
    pub title: String,
    /// A longer human summary.
    pub summary: String,
    /// The verbs this endpoint answers.
    pub verbs: Vec<Verb>,
    /// The named inputs it accepts.
    pub inputs: Vec<ArgSpec>,
    /// The representation types it can produce (canonical media-type strings).
    pub outputs: Vec<String>,
    /// What kind of endpoint this is. Defaults to [`EndpointKind::Endpoint`]; omitted from
    /// serialized output for plain endpoints, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "EndpointKind::is_endpoint")]
    pub kind: EndpointKind,
    /// The capability scopes invoking this endpoint requires (descriptive labels, e.g.
    /// `cap:net`, `cap:fs:read`) — what *authority* it demands, not a runtime
    /// [`Capability`](crate::Capability). Lets a host project a capability-scoped catalog
    /// (show an agent only what it may invoke) and lets a caller pre-check feasibility.
    /// Empty for endpoints needing no special authority; omitted from serialized output when
    /// empty, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// What *kind* of endpoint this is — a first-class type that projects to an RDF class
/// (`ik:Endpoint`, `ik:Transreptor`, …) and lets the kernel select endpoints by role.
///
/// The default, [`Endpoint`](EndpointKind::Endpoint), is a plain endpoint (today's
/// behaviour). [`Transreptor`](EndpointKind::Transreptor) marks an endpoint that converts a
/// representation between media types and carries the `from`/`to` types it handles, so a host
/// can find "a transreptor from A to B." More kinds may follow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EndpointKind {
    /// A plain endpoint (`ik:Endpoint`).
    #[default]
    Endpoint,
    /// A transreptor (`ik:Transreptor ⊏ ik:Endpoint`) — converts a representation from one
    /// media type to another, carrying the conversions it supports.
    Transreptor(Transreption),
}

impl EndpointKind {
    /// Whether this is the default plain [`Endpoint`](EndpointKind::Endpoint) kind.
    pub fn is_endpoint(&self) -> bool {
        matches!(self, EndpointKind::Endpoint)
    }
}

/// The media-type conversions a transreptor supports: it can accept any of `from` and
/// produce any of `to`. Used both to type the endpoint in RDF (`ik:transreptsFrom`/`To`) and
/// to select a transreptor for a needed `from → to` conversion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Transreption {
    /// The media types it can read.
    pub from: Vec<String>,
    /// The media types it can produce.
    pub to: Vec<String>,
}

impl Description {
    /// A description for the endpoint with the given identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Description {
            id: id.into(),
            ..Default::default()
        }
    }

    /// Set the title (builder).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the summary (builder).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Declare a supported verb (builder).
    pub fn verb(mut self, verb: Verb) -> Self {
        self.verbs.push(verb);
        self
    }

    /// Declare an input (builder).
    pub fn input(mut self, input: ArgSpec) -> Self {
        self.inputs.push(input);
        self
    }

    /// Declare an output media type (builder).
    pub fn output(mut self, media_type: impl Into<String>) -> Self {
        self.outputs.push(media_type.into());
        self
    }

    /// Declare a capability scope this endpoint requires to be invoked (builder), e.g.
    /// `cap:net`. Callable multiple times; descriptive only (drives capability-scoped
    /// catalog projection and feasibility pre-checks), not runtime enforcement.
    pub fn requires(mut self, capability: impl Into<String>) -> Self {
        self.requires.push(capability.into());
        self
    }

    /// Set the endpoint kind (builder).
    pub fn kind(mut self, kind: EndpointKind) -> Self {
        self.kind = kind;
        self
    }

    /// Mark this endpoint a **transreptor** that converts representations from any of `from`
    /// to any of `to` (media-type strings) — builder. Sets [`EndpointKind::Transreptor`].
    pub fn transreptor(
        mut self,
        from: impl IntoIterator<Item = impl Into<String>>,
        to: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.kind = EndpointKind::Transreptor(Transreption {
            from: from.into_iter().map(Into::into).collect(),
            to: to.into_iter().map(Into::into).collect(),
        });
        self
    }

    /// The transreption this endpoint supports, if it is a transreptor — for the vocabulary
    /// projection and for transreptor selection.
    pub fn transreption(&self) -> Option<&Transreption> {
        match &self.kind {
            EndpointKind::Transreptor(t) => Some(t),
            EndpointKind::Endpoint => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_description() {
        let d = Description::new("toUpper")
            .title("Upper-case")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").summary("the string").optional())
            .output("text/plain");
        assert_eq!(d.id, "toUpper");
        assert_eq!(d.verbs, vec![Verb::Source]);
        assert_eq!(d.inputs[0].name, "in");
        assert!(!d.inputs[0].required);
    }

    #[test]
    fn inputs_default_to_arguments_and_can_be_bindings() {
        let arg = ArgSpec::new("in");
        assert_eq!(arg.source, InputSource::Argument);
        let bound = ArgSpec::new("message").binding();
        assert_eq!(bound.source, InputSource::Binding);
    }

    #[test]
    fn serde_round_trip() {
        let d = Description::new("x")
            .verb(Verb::Meta)
            .input(ArgSpec::new("message").binding());
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), d);
    }

    #[test]
    fn kind_defaults_to_endpoint_and_is_omitted_from_json() {
        let d = Description::new("toUpper").output("text/plain");
        assert_eq!(d.kind, EndpointKind::Endpoint);
        assert!(d.transreption().is_none());
        // A plain endpoint's JSON carries no `kind` key — the engine's contract is unchanged.
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("kind"), "{json}");
        // …and JSON without a `kind` still deserializes (back-compat).
        let d2: Description = serde_json::from_str(&json).unwrap();
        assert_eq!(d2.kind, EndpointKind::Endpoint);
    }

    #[test]
    fn transreptor_builder_records_its_conversions() {
        let d = Description::new("rdf-transrept")
            .verb(Verb::Source)
            .transreptor(
                ["text/turtle", "application/rdf+xml"],
                ["text/turtle", "text/html"],
            );
        let t = d.transreption().expect("is a transreptor");
        assert_eq!(t.from, vec!["text/turtle", "application/rdf+xml"]);
        assert_eq!(t.to, vec!["text/turtle", "text/html"]);
        assert!(!d.kind.is_endpoint());
        // The transreptor kind round-trips through serde.
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("transreptor"), "{json}");
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), d);
    }

    #[test]
    fn input_class_is_optional_and_omitted_when_absent() {
        // No class → no `class` key in the input's JSON (existing contract unchanged).
        let plain = ArgSpec::new("content");
        assert_eq!(plain.class, None);
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("class"), "{json}");

        // With a class → recorded and round-trips.
        let typed = ArgSpec::new("who").class("https://schema.org/Person");
        assert_eq!(typed.class.as_deref(), Some("https://schema.org/Person"));
        assert_eq!(
            serde_json::from_str::<ArgSpec>(&serde_json::to_string(&typed).unwrap()).unwrap(),
            typed
        );
    }

    #[test]
    fn requires_is_empty_by_default_and_omitted_from_json() {
        let d = Description::new("toUpper").output("text/plain");
        assert!(d.requires.is_empty());
        // A no-authority endpoint carries no `requires` key — JSON contract unchanged.
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("requires"), "{json}");

        // Declared capability scopes accumulate and round-trip.
        let gated = Description::new("httpGet")
            .verb(Verb::Source)
            .requires("cap:net")
            .requires("cap:fs:read");
        assert_eq!(gated.requires, vec!["cap:net", "cap:fs:read"]);
        assert_eq!(
            serde_json::from_str::<Description>(&serde_json::to_string(&gated).unwrap()).unwrap(),
            gated
        );
    }

    #[test]
    fn a_plain_descriptions_json_is_byte_identical_to_before_these_fields() {
        // Belt-and-suspenders: an endpoint using neither new field serializes with exactly
        // the pre-existing keys — so the engine's machine contract is untouched.
        let d = Description::new("toUpper")
            .title("Upper-case")
            .verb(Verb::Source)
            .input(ArgSpec::new("in"))
            .output("text/plain");
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            !json.contains("class") && !json.contains("requires"),
            "{json}"
        );
    }
}
