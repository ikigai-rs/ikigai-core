//! The ikigai self-description vocabulary and its RDF (Turtle) projection.
//!
//! Endpoints describe themselves with a neutral [`Description`] (in
//! `ikigai-core`); this crate renders that to RDF so a `Meta` request can return
//! a machine-readable description of an endpoint.
//!
//! The Turtle is hand-rolled, which keeps this crate dependency-free and
//! WASM-trivial — the emitted graph is small and fully controlled. (A future
//! revision may project to Hydra / OpenAPI from the same vocabulary.)

use ikigai_core::{
    Description, EndpointSpace, Error, Exact, FnEndpoint, InputSource, Invocation, MetaRenderer,
    ReprType, Representation, Result, Verb,
};

/// The ikigai vocabulary namespace. Provisional — the canonical namespace IRI
/// is a project decision; it is used here purely as a stable identifier.
pub const NS: &str = "https://ikigai-rs.dev/ns#";

/// The ikigai vocabulary itself, as a Turtle ontology — hand-maintained in
/// `src/vocabulary.ttl` and bundled at compile time. Defines the `ns#` classes
/// (`ik:Endpoint`, `ik:Transreptor ⊏ ik:Endpoint`, …) and properties. Served by
/// [`space`] at `urn:ikigai:vocab`, and eventually at the external `ns#` URL.
pub const VOCABULARY: &str = include_str!("vocabulary.ttl");

/// The conventional IRI the vocabulary is bound to by [`space`].
pub const VOCAB_IRI: &str = "urn:ikigai:vocab";

/// Escape a string for use inside a Turtle double-quoted literal.
fn escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn lit(s: &str) -> String {
    format!("\"{}\"", escape_literal(s))
}

fn verb_name(verb: Verb) -> &'static str {
    match verb {
        Verb::Source => "Source",
        Verb::Sink => "Sink",
        Verb::Exists => "Exists",
        Verb::Delete => "Delete",
        Verb::Meta => "Meta",
    }
}

fn source_name(source: InputSource) -> &'static str {
    match source {
        InputSource::Argument => "argument",
        InputSource::Binding => "binding",
    }
}

/// Render a [`Description`] as a Turtle document using the ikigai vocabulary.
pub fn to_turtle(description: &Description) -> String {
    // `id` is a resource identifier (no Turtle-significant characters).
    let subject = format!("<urn:ikigai:endpoint:{}>", description.id);

    // A transreptor is typed as both classes explicitly (`ik:Transreptor ⊏ ik:Endpoint`),
    // so consumers that don't reason over the subclass axiom still see `ik:Endpoint`.
    let rdf_type = if description.transreption().is_some() {
        "a ik:Endpoint, ik:Transreptor"
    } else {
        "a ik:Endpoint"
    };
    let mut predicates: Vec<String> = vec![
        rdf_type.to_string(),
        format!("ik:id {}", lit(&description.id)),
    ];
    if !description.title.is_empty() {
        predicates.push(format!("ik:title {}", lit(&description.title)));
    }
    if !description.summary.is_empty() {
        predicates.push(format!("ik:summary {}", lit(&description.summary)));
    }
    if !description.verbs.is_empty() {
        let verbs = description
            .verbs
            .iter()
            .map(|v| lit(verb_name(*v)))
            .collect::<Vec<_>>()
            .join(", ");
        predicates.push(format!("ik:verb {verbs}"));
    }
    if !description.outputs.is_empty() {
        let outputs = description
            .outputs
            .iter()
            .map(|o| lit(o))
            .collect::<Vec<_>>()
            .join(", ");
        predicates.push(format!("ik:output {outputs}"));
    }
    if let Some(t) = description.transreption() {
        if !t.from.is_empty() {
            let from = t.from.iter().map(|m| lit(m)).collect::<Vec<_>>().join(", ");
            predicates.push(format!("ik:transreptsFrom {from}"));
        }
        if !t.to.is_empty() {
            let to = t.to.iter().map(|m| lit(m)).collect::<Vec<_>>().join(", ");
            predicates.push(format!("ik:transreptsTo {to}"));
        }
    }
    for input in &description.inputs {
        let mut node = format!(
            "[ ik:inputName {} ; ik:source {} ; ik:required {}",
            lit(&input.name),
            lit(source_name(input.source)),
            input.required
        );
        if !input.summary.is_empty() {
            node.push_str(&format!(" ; ik:summary {}", lit(&input.summary)));
        }
        node.push_str(" ]");
        predicates.push(format!("ik:input {node}"));
    }

    format!(
        "@prefix ik: <{NS}> .\n\n{subject} {} .\n",
        predicates.join(" ;\n    ")
    )
}

/// Render a [`Description`] as a `text/turtle` [`Representation`].
pub fn describe_turtle(description: &Description) -> Representation {
    Representation::new(
        ReprType::new("text/turtle"),
        to_turtle(description).into_bytes(),
    )
}

/// A space binding [`VOCAB_IRI`] (`urn:ikigai:vocab`) to the [`VOCABULARY`] Turtle. Mount it
/// in a kernel's root so the vocabulary is `source`-able as a resource — and, via the http
/// arc, servable at the external `ns#` URL. (Cacheable; lists in the catalog as a plain
/// endpoint.)
pub fn space() -> EndpointSpace {
    EndpointSpace::new().bind(
        Exact::new(VOCAB_IRI),
        FnEndpoint::new("ikigai-vocab", |_inv: &Invocation<'_>| {
            Ok(Representation::new(
                ReprType::new("text/turtle").with_param("charset", "utf-8"),
                VOCABULARY.as_bytes().to_vec(),
            )
            .cacheable())
        })
        .with_description(
            Description::new("ikigai-vocab")
                .title("ikigai vocabulary")
                .summary(
                    "The ikigai self-description vocabulary (the ns# ontology): the endpoint and \
                     transreptor classes and their properties.",
                )
                .verb(Verb::Source)
                .verb(Verb::Meta)
                .output("text/turtle;charset=utf-8"),
        ),
    )
}

/// Render a [`Description`] as human-readable plain text (for the CLI `describe`).
pub fn to_text(description: &Description) -> String {
    let mut s = format!("{} — {}\n", description.id, description.title);
    if !description.summary.is_empty() {
        s.push_str(&format!("{}\n", description.summary));
    }
    if !description.verbs.is_empty() {
        let verbs: Vec<&str> = description.verbs.iter().map(|v| verb_name(*v)).collect();
        s.push_str(&format!("verbs: {}\n", verbs.join(", ")));
    }
    for input in &description.inputs {
        let opt = if input.required { "" } else { " (optional)" };
        s.push_str(&format!(
            "  input {} [{}]{}: {}\n",
            input.name,
            source_name(input.source),
            opt,
            input.summary
        ));
    }
    if !description.outputs.is_empty() {
        s.push_str(&format!("outputs: {}\n", description.outputs.join(", ")));
    }
    if let Some(t) = description.transreption() {
        s.push_str(&format!(
            "transrepts: {} → {}\n",
            t.from.join(", "),
            t.to.join(", ")
        ));
    }
    s
}

/// A [`MetaRenderer`] projecting descriptions to `text/turtle` (the default) or
/// `text/plain`. Inject it into a kernel via `Kernel::with_meta_renderer`.
pub struct TurtleRenderer;

impl MetaRenderer for TurtleRenderer {
    fn render(&self, description: &Description, target: &ReprType) -> Result<Representation> {
        match target.media_type.as_str() {
            "text/turtle" | "*/*" | "" => Ok(describe_turtle(description)),
            "text/plain" => Ok(Representation::new(
                ReprType::new("text/plain").with_param("charset", "utf-8"),
                to_text(description).into_bytes(),
            )),
            other => Err(Error::Endpoint(format!(
                "meta renderer does not support target `{other}`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ikigai_core::{ArgSpec, Space};

    fn sample() -> Description {
        Description::new("toUpper")
            .title("Upper-case")
            .summary("Upper-cases its input.")
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(ArgSpec::new("in").summary("the string"))
            .output("text/plain;charset=utf-8")
    }

    #[test]
    fn renders_expected_triples() {
        let ttl = to_turtle(&sample());
        assert!(ttl.starts_with("@prefix ik: <https://ikigai-rs.dev/ns#> ."));
        assert!(ttl.contains("a ik:Endpoint"));
        assert!(ttl.contains("ik:id \"toUpper\""));
        assert!(ttl.contains("ik:verb \"Source\", \"Meta\""));
        assert!(ttl.contains("ik:output \"text/plain;charset=utf-8\""));
        assert!(ttl.contains("ik:inputName \"in\""));
        assert!(ttl.contains("ik:source \"argument\""));
        assert!(ttl.contains("ik:required true"));
        assert!(ttl.trim_end().ends_with('.'));
    }

    #[test]
    fn renders_binding_inputs() {
        let d = Description::new("echo").verb(Verb::Source).input(
            ArgSpec::new("message")
                .summary("captured from the path")
                .binding(),
        );
        let ttl = to_turtle(&d);
        assert!(ttl.contains("ik:inputName \"message\""));
        assert!(ttl.contains("ik:source \"binding\""));
        let text = to_text(&d);
        assert!(text.contains("input message [binding]"));
    }

    #[test]
    fn escapes_literals() {
        let d = Description::new("x").title("a \"quote\" and \\ slash");
        let ttl = to_turtle(&d);
        assert!(ttl.contains(r#"ik:title "a \"quote\" and \\ slash""#));
    }

    #[test]
    fn describe_turtle_is_text_turtle() {
        let rep = describe_turtle(&sample());
        assert_eq!(rep.repr_type.media_type, "text/turtle");
    }

    /// Parse Turtle and return the triple count, panicking on any syntax error.
    fn parse_count(ttl: &str) -> usize {
        let mut count = 0;
        for triple in oxttl::TurtleParser::new().for_reader(ttl.as_bytes()) {
            triple.expect("valid turtle");
            count += 1;
        }
        count
    }

    #[test]
    fn renders_a_transreptor() {
        let d = Description::new("rdf-transrept").verb(Verb::Source).transreptor(
            ["text/turtle", "application/rdf+xml"],
            ["text/turtle", "text/html"],
        );
        let ttl = to_turtle(&d);
        assert!(ttl.contains("a ik:Endpoint, ik:Transreptor"), "{ttl}");
        assert!(
            ttl.contains("ik:transreptsFrom \"text/turtle\", \"application/rdf+xml\""),
            "{ttl}"
        );
        assert!(ttl.contains("ik:transreptsTo \"text/turtle\", \"text/html\""), "{ttl}");
        assert!(parse_count(&ttl) > 0, "emitted transreptor turtle parses");
        // A plain endpoint stays just ik:Endpoint.
        let plain = to_turtle(&sample());
        assert!(plain.contains("a ik:Endpoint") && !plain.contains("ik:Transreptor"), "{plain}");
    }

    #[test]
    fn bundled_vocabulary_is_valid_turtle_with_the_subclass_axiom() {
        assert!(parse_count(VOCABULARY) > 0, "vocabulary.ttl parses");
        assert!(VOCABULARY.contains("ik:Transreptor"));
        assert!(VOCABULARY.contains("rdfs:subClassOf ik:Endpoint"));
        assert!(VOCABULARY.contains("ik:transreptsFrom") && VOCABULARY.contains("ik:transreptsTo"));
    }

    #[test]
    fn vocab_space_binds_the_vocab_iri() {
        let entries = space().entries().expect("space enumerates");
        assert!(entries.iter().any(|e| e.pattern == VOCAB_IRI), "{entries:?}");
    }
}
