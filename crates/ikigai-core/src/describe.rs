use serde::{Deserialize, Serialize};

use crate::verb::Verb;

/// A specification of one named input an endpoint accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgSpec {
    /// The argument name.
    pub name: String,
    /// A short human description.
    pub summary: String,
    /// Whether the argument is required.
    pub required: bool,
}

impl ArgSpec {
    /// A required argument with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        ArgSpec {
            name: name.into(),
            summary: String::new(),
            required: true,
        }
    }

    /// Add a human summary (builder).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Mark the argument optional (builder).
    pub fn optional(mut self) -> Self {
        self.required = false;
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
    fn serde_round_trip() {
        let d = Description::new("x").verb(Verb::Meta);
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), d);
    }
}
