//! Transreptor selection — find a chain of transreptors that converts a representation from
//! one media type to another, using the `from`/`to` each transreptor declares
//! ([`EndpointKind::Transreptor`](crate::EndpointKind)).
//!
//! This is the kernel capability metadata rendering, content-negotiation, and
//! sniff-and-dispatch all build on: "give me a way to get from media type A to B." v1 finds
//! a **direct single hop**, else a **two-hop pivot via the canonical RDF type
//! (`text/turtle`)** — the hub our transreptors share. (General N-hop path-finding, and
//! caching the transreptor index, are later refinements; today it enumerates per call.)
//!
//! Only **auto-invocable** transreptors are selected — ones drivable with just a piped
//! `content` and a target `as` (see [`is_auto_invocable`]). A *parameterized* transreptor
//! like `urn:xslt:transform` (which requires a `stylesheet`) is still a transreptor for
//! discovery, but can't be invoked automatically, so it's excluded here.

use std::sync::Arc;

use crate::describe::Description;
use crate::iri::Iri;
use crate::request::Request;
use crate::space::{Resolution, Scope, Space};
use crate::verb::Verb;

/// The canonical RDF media type the transreptor graph hubs on — the pivot for two-hop
/// conversions when no direct transreptor exists.
pub const CANONICAL: &str = "text/turtle";

/// One step of a transreption plan: invoke the transreptor at `endpoint` with the input
/// piped as `content` and `as` set to `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransreptionStep {
    /// The transreptor endpoint's IRI.
    pub endpoint: String,
    /// The media type to request from it (its `as`).
    pub to: String,
}

/// A transreptor's declared conversions, paired with its IRI.
struct Candidate {
    iri: String,
    from: Vec<String>,
    to: Vec<String>,
}

impl Candidate {
    fn handles(&self, from: &str, to: &str) -> bool {
        self.from.iter().any(|f| f == from) && self.to.iter().any(|t| t == to)
    }
}

/// Whether a transreptor can be invoked automatically — driven with only a piped `content`
/// and a target `as`. True iff every *required* input is `content` or `as`. A transreptor
/// with another required input (e.g. `urn:xslt:transform`'s `stylesheet`) is parameterized
/// and must be invoked explicitly, so it is not auto-selected.
pub fn is_auto_invocable(description: &Description) -> bool {
    description
        .inputs
        .iter()
        .filter(|i| i.required)
        .all(|i| i.name == "content" || i.name == "as")
}

/// Find a chain of auto-invocable transreptors in `root` converting `from` → `to`: a direct
/// single hop if one exists, else a two-hop pivot via [`CANONICAL`]. `None` if no chain is
/// available (or if `from == to`, which needs no transreption).
pub fn select_transreptor(root: &dyn Space, from: &str, to: &str) -> Option<Vec<TransreptionStep>> {
    if from == to {
        return None;
    }
    let candidates = collect(root);

    // Direct: a single transreptor that reads `from` and produces `to`.
    if let Some(c) = candidates.iter().find(|c| c.handles(from, to)) {
        return Some(vec![TransreptionStep {
            endpoint: c.iri.clone(),
            to: to.to_string(),
        }]);
    }

    // Pivot: `from → text/turtle` then `text/turtle → to` (two distinct hops).
    if from != CANONICAL && to != CANONICAL {
        let first = candidates.iter().find(|c| c.handles(from, CANONICAL))?;
        let second = candidates.iter().find(|c| c.handles(CANONICAL, to))?;
        return Some(vec![
            TransreptionStep {
                endpoint: first.iri.clone(),
                to: CANONICAL.to_string(),
            },
            TransreptionStep {
                endpoint: second.iri.clone(),
                to: to.to_string(),
            },
        ]);
    }

    None
}

/// Enumerate `root`'s auto-invocable transreptors (the same `entries → Meta → describe`
/// walk the catalog uses).
fn collect(root: &dyn Space) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for entry in root.entries().unwrap_or_default() {
        let Ok(iri) = Iri::parse(&entry.pattern) else {
            continue;
        };
        if let Resolution::Hit(resolved) =
            root.resolve(&Request::new(Verb::Meta, iri), &Scope::empty())
        {
            let description = resolved.endpoint.describe();
            if let Some(t) = description.transreption() {
                if is_auto_invocable(&description) {
                    candidates.push(Candidate {
                        iri: entry.pattern.clone(),
                        from: t.from.clone(),
                        to: t.to.clone(),
                    });
                }
            }
        }
    }
    candidates
}

/// Convenience: select over an `Arc<dyn Space>` root (as a kernel holds).
pub fn select_transreptor_in(
    root: &Arc<dyn Space>,
    from: &str,
    to: &str,
) -> Option<Vec<TransreptionStep>> {
    select_transreptor(root.as_ref(), from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::describe::Description;
    use crate::endpoint::{Endpoint, FnEndpoint};
    use crate::grammar::Exact;
    use crate::repr::{ReprType, Representation};
    use crate::space::EndpointSpace;

    /// A stub transreptor: declares from/to (auto-invocable: content + as), does nothing.
    fn transreptor(id: &'static str, from: &[&str], to: &[&str]) -> FnEndpoint {
        FnEndpoint::new(id, |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new(id)
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("content"))
                .input(crate::describe::ArgSpec::new("as"))
                .transreptor(from.iter().copied(), to.iter().copied()),
        )
    }

    fn space() -> EndpointSpace {
        EndpointSpace::new()
            // turtle <-> rdf/xml, n-triples, html (an rdf-transrept-like hub)
            .bind(
                Exact::new("urn:rdf:transrept"),
                transreptor(
                    "rdf",
                    &[
                        "text/turtle",
                        "application/rdf+xml",
                        "application/n-triples",
                    ],
                    &[
                        "text/turtle",
                        "application/rdf+xml",
                        "application/n-triples",
                        "text/html",
                    ],
                ),
            )
            // a plain (non-transreptor) endpoint — must be ignored
            .bind(
                Exact::new("urn:fn:toUpper"),
                FnEndpoint::new("toUpper", |_inv| {
                    Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
                }),
            )
    }

    #[test]
    fn finds_a_direct_hop() {
        let plan = select_transreptor(&space(), "application/rdf+xml", "text/turtle").unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].endpoint, "urn:rdf:transrept");
        assert_eq!(plan[0].to, "text/turtle");
    }

    #[test]
    fn pivots_via_turtle_when_no_direct_hop() {
        // rdf/xml → html: no single transreptor declares that pair directly here? It does
        // (rdf handles rdf+xml→html). Use a case needing the pivot: add a turtle→csv-only
        // transreptor and ask rdf+xml → text/csv.
        let space = space().bind(
            Exact::new("urn:demo:csv"),
            transreptor("csv", &["text/turtle"], &["text/csv"]),
        );
        let plan = select_transreptor(&space, "application/rdf+xml", "text/csv").unwrap();
        assert_eq!(plan.len(), 2, "{plan:?}");
        assert_eq!(plan[0].to, "text/turtle"); // pivot
        assert_eq!(plan[1].endpoint, "urn:demo:csv");
        assert_eq!(plan[1].to, "text/csv");
    }

    #[test]
    fn none_when_unreachable_or_identity() {
        assert!(select_transreptor(&space(), "text/turtle", "text/turtle").is_none());
        assert!(select_transreptor(&space(), "application/pdf", "image/png").is_none());
    }

    #[test]
    fn parameterized_transreptors_are_not_auto_invocable() {
        // An xslt-like transreptor with a required `stylesheet` is excluded from selection.
        let xslt = FnEndpoint::new("xslt", |_inv| {
            Ok(Representation::new(ReprType::new("text/html"), Vec::new()))
        })
        .with_description(
            Description::new("xslt")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("content"))
                .input(crate::describe::ArgSpec::new("stylesheet"))
                .input(crate::describe::ArgSpec::new("as"))
                .transreptor(["application/xml"], ["text/html"]),
        );
        assert!(!is_auto_invocable(&xslt.describe()));
        let space = EndpointSpace::new().bind(Exact::new("urn:xslt:transform"), xslt);
        assert!(select_transreptor(&space, "application/xml", "text/html").is_none());
    }
}
