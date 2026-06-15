//! The ikigai self-description vocabulary and its RDF (Turtle) projection.
//!
//! Endpoints describe themselves with a neutral [`Description`] (in
//! `ikigai-core`); this crate renders that to RDF so a `Meta` request can return
//! a machine-readable description of an endpoint.
//!
//! The Turtle is hand-rolled, which keeps this crate dependency-free and
//! WASM-trivial — the emitted graph is small and fully controlled. (A future
//! revision may project to Hydra / OpenAPI from the same vocabulary.)

use ikigai_core::{Description, Error, MetaRenderer, ReprType, Representation, Result, Verb};

/// The ikigai vocabulary namespace. Provisional — the canonical namespace IRI
/// is a project decision; it is used here purely as a stable identifier.
pub const NS: &str = "https://ikigai-rs.dev/ns#";

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

/// Render a [`Description`] as a Turtle document using the ikigai vocabulary.
pub fn to_turtle(description: &Description) -> String {
    // `id` is a resource identifier (no Turtle-significant characters).
    let subject = format!("<urn:ikigai:endpoint:{}>", description.id);

    let mut predicates: Vec<String> = vec![
        "a ik:Endpoint".to_string(),
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
    for input in &description.inputs {
        let mut node = format!(
            "[ ik:inputName {} ; ik:required {}",
            lit(&input.name),
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
            "  input {}{}: {}\n",
            input.name, opt, input.summary
        ));
    }
    if !description.outputs.is_empty() {
        s.push_str(&format!("outputs: {}\n", description.outputs.join(", ")));
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
    use ikigai_core::ArgSpec;

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
        assert!(ttl.contains("ik:required true"));
        assert!(ttl.trim_end().ends_with('.'));
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
}
