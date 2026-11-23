//! Checks the Turtle example and the SPARQL query in `docs/design/constraint-history.md`:
//! loads the example interval and two copies shifted by 15 and 30 minutes, runs the query,
//! and prints its rows. Expected: one row, books-graph, 3 intervals led, 1194336 ms.
//!
//!     cargo run --release --manifest-path docs/design/constraint-history/measure/Cargo.toml \
//!         --bin check-query -- docs/design/constraint-history.md

use oxigraph::io::RdfFormat;
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
fn block(md: &str, lang: &str) -> String {
    let start = md.find(&format!("```{lang}\n")).unwrap() + lang.len() + 4;
    let end = start + md[start..].find("```").unwrap();
    md[start..end].to_string()
}
fn main() {
    let md = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let ttl = block(&md, "turtle");
    let query = block(&md, "sparql");
    let store = Store::new().unwrap();
    // The example interval, and two more shifted by 15 and 30 minutes.
    for (i, (until_ms, since, until)) in [
        ("1795161600000", "07:45", "08:00"),
        ("1795162500000", "08:00", "08:15"),
        ("1795163400000", "08:15", "08:30"),
    ]
    .iter()
    .enumerate()
    {
        let t = ttl
            .replace("1795161600000", until_ms)
            .replace("2026-11-20T07:45:00Z", &format!("2026-11-20T{since}:00Z"))
            .replace("2026-11-20T08:00:00Z", &format!("2026-11-20T{until}:00Z"));
        if i == 0 {
            assert_eq!(t, ttl);
        }
        store
            .load_from_slice(RdfFormat::Turtle, t.as_bytes())
            .unwrap();
    }
    println!("triples: {}", store.len().unwrap());
    let QueryResults::Solutions(rows) = SparqlEvaluator::new()
        .parse_query(&query)
        .unwrap()
        .on_store(&store)
        .execute()
        .unwrap()
    else {
        panic!()
    };
    for row in rows {
        println!(
            "{}",
            row.unwrap()
                .iter()
                .map(|(v, t)| format!("{v}={t}"))
                .collect::<Vec<_>>()
                .join("  ")
        );
    }
}
