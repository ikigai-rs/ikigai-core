use ikigai_core::{escape_literal, turtle_literal};

#[test]
fn turtle_short_string_vectors() {
    for (input, escaped) in [
        ("", ""),
        ("plain", "plain"),
        ("\"", "\\\""),
        ("\\", "\\\\"),
        ("\n", "\\n"),
        ("\r", "\\r"),
        ("\t", "\t"),
        ("é", "é"),
        ("\\\"\n\r\té", "\\\\\\\"\\n\\r\té"),
    ] {
        assert_eq!(escape_literal(input), escaped);
        let quoted = turtle_literal(input);
        assert_eq!(quoted, format!("\"{escaped}\""));
        let triples = oxttl::TurtleParser::new()
            .for_reader(format!("<urn:s> <urn:p> {quoted} .").as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let oxrdf::Term::Literal(literal) = &triples[0].object else {
            panic!("expected a literal");
        };
        assert_eq!(literal.value(), input);
    }
}
