//! Pins TODAY's behavior that `docs/design/describe-outcomes.md` proposes to change (ledger
//! #4353). Each test states a gap the design closes; none of them is the desired behavior.
//! When the implementation lands, these assertions flip, deliberately.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    describe_entry, Capability, Description, EndpointSpace, Exact, Expiry, Fallback, FnEndpoint,
    Iri, Kernel, Limit, MetaRenderer, ReprType, Representation, Request, Resolution, Resolved,
    Scope, Space, SpaceEntry, UriTemplate, Verb,
};

struct IdRenderer;
impl MetaRenderer for IdRenderer {
    fn render(&self, d: &Description, _: &ReprType) -> ikigai_core::Result<Representation> {
        Ok(Representation::new(
            ReprType::new("text/turtle"),
            d.id.as_bytes().to_vec(),
        ))
    }
}

fn door(id: &'static str, description: Description) -> FnEndpoint {
    FnEndpoint::new(id, move |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            id.as_bytes().to_vec(),
        ))
    })
    .with_description(description)
}

/// A space that resolves one door but cannot enumerate (`entries` is the default `None`),
/// the shape of a rewrite or a remote whose listing is unavailable.
struct Opaque(Arc<FnEndpoint>);
impl Space for Opaque {
    fn resolve(&self, request: &Request, _: &Scope) -> Resolution {
        if request.target.as_str() == "urn:opaque:door" {
            Resolution::Hit(Resolved::new(self.0.clone(), Default::default()))
        } else {
            Resolution::Miss
        }
    }
}

fn read(kernel: &Kernel, target: &str) -> Representation {
    block_on(kernel.issue(
        Request::new(Verb::Source, Iri::parse(target).unwrap()),
        &Capability::root(),
    ))
    .unwrap()
}

fn text(repr: &Representation) -> String {
    String::from_utf8(repr.bytes.clone()).unwrap()
}

/// Gap: enumeration completeness is not carried through `Fallback`. One member enumerates,
/// so the fallback answers `Some`, and the opaque member's door is reachable by resolution
/// yet absent from both faces, with nothing saying the walk was partial.
#[test]
fn an_opaque_member_vanishes_beside_an_enumerable_one() {
    let opaque = Arc::new(door(
        "urn:opaque:door",
        Description::new("urn:opaque:door").verb(Verb::Source),
    ));
    let space = Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:listed:door"),
            door(
                "urn:listed:door",
                Description::new("urn:listed:door").verb(Verb::Source),
            ),
        )),
        Arc::new(Opaque(opaque)),
    ]);
    assert_eq!(space.entries().map(|e| e.len()), Some(1));
    let kernel = Kernel::with_meta_renderer(Arc::new(space), Arc::new(IdRenderer));

    // Reachable: the door answers.
    assert_eq!(text(&read(&kernel, "urn:opaque:door")), "urn:opaque:door");

    let catalog = text(&read(&kernel, "urn:kernel:catalog"));
    let actions = text(&read(&kernel, "urn:kernel:actions"));
    assert!(catalog.contains("urn:listed:door") && actions.contains("urn:listed:door"));
    assert!(!catalog.contains("urn:opaque:door"));
    assert!(!actions.contains("urn:opaque:door"));
    // No incompleteness marker of any kind on either face.
    for face in [&catalog, &actions] {
        assert!(!face.contains("incomplete") && !face.contains("Incomplete"));
    }
}

/// Gap: a wholly opaque root still yields a manifold, because the kernel walks
/// `Fallback([kernel operations, root])` and the kernel operations enumerate. The root's
/// doors are missing and the answer looks complete.
#[test]
fn an_opaque_root_leaves_a_manifold_of_kernel_operations_only() {
    let opaque = Arc::new(door(
        "urn:opaque:door",
        Description::new("urn:opaque:door").verb(Verb::Source),
    ));
    let kernel = Kernel::with_meta_renderer(Arc::new(Opaque(opaque)), Arc::new(IdRenderer));
    let actions = text(&read(&kernel, "urn:kernel:actions"));
    assert!(actions.contains("urn:kernel:"));
    assert!(!actions.contains("urn:opaque:door"));
}

/// Gap: `describe_entry` answers `None` for distinct reasons a caller cannot tell apart: a
/// pattern that is neither an IRI nor a template, a probe that misses, a limiter hit, and an
/// identity mismatch on a template probe.
#[test]
fn describe_entry_collapses_distinct_reasons_into_none() {
    let space = Fallback::new(vec![
        Arc::new(Limit::new("urn:limited:")),
        Arc::new(
            EndpointSpace::new()
                .bind(
                    Exact::new("urn:limited:door"),
                    door("urn:limited:door", Description::new("urn:limited:door")),
                )
                // Binds first, so a probe of the template below lands here.
                .bind(
                    UriTemplate::parse("urn:shared:{x}").unwrap(),
                    door("first", Description::new("first")),
                ),
        ),
    ]);
    let cases = [
        // Neither an IRI nor a template.
        SpaceEntry::new("not an iri", "anything"),
        // A parseable IRI bound nowhere: a confirmed miss.
        SpaceEntry::new("urn:nowhere:door", "nowhere"),
        // A name the limiter carved out.
        SpaceEntry::new("urn:limited:door", "urn:limited:door"),
        // A template whose probe lands on a different binding.
        SpaceEntry::new("urn:shared:{y}", "second"),
    ];
    for entry in &cases {
        assert_eq!(describe_entry(&space, entry), None, "{}", entry.pattern);
    }
}

/// Gap: a placeholder standing for a failed metadata fetch is cached until rebind unless
/// the adapter remembers to mark it volatile. The manifold folds only the contracts' own
/// caching, so the transient failure is kept as a durable answer.
#[test]
fn a_placeholder_not_marked_volatile_makes_the_manifold_cacheable_until_rebind() {
    let build = |placeholder: Description| {
        let space = EndpointSpace::new()
            .bind(
                Exact::new("urn:peer:slow"),
                door("urn:peer:slow", placeholder),
            )
            .bind(
                Exact::new("urn:peer:ready"),
                door(
                    "urn:peer:ready",
                    Description::new("urn:peer:ready").verb(Verb::Source),
                ),
            );
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(IdRenderer))
    };
    let forgot = build(Description::new("urn:peer:slow"));
    let marked = build(Description::new("urn:peer:slow").contract_volatile());
    for target in ["urn:kernel:actions", "urn:kernel:catalog"] {
        assert_eq!(read(&forgot, target).expiry, Expiry::Never, "{target}");
        assert_eq!(read(&marked, target).expiry, Expiry::Always, "{target}");
    }
}
