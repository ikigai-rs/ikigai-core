//! **A space declared as data** — the inverse of `urn:kernel:topology` (ledger #633).
//!
//! The acceptance test is the ROUND TRIP. For each kernel K below, covering every
//! kind a declaration can rebuild: render K's arrangement, read it back, build K′
//! from the same registry of endpoints, and require
//!
//! - a fixpoint: `topology(K′) == topology(K)`, as a tree and as Turtle; and
//! - the same answers: for a sample of names drawn from the arrangement itself (every
//!   exact door, an instance of every template, every alias source, names under every
//!   limiter, mount and seal, and names that miss), K′ resolves each exactly as K
//!   does — the same endpoint name, bindings, `answered_by`, canonical name and level
//!   path — and the kernel over K′ answers each with the same bytes or the same
//!   refusal.
//!
//! Then the refusals: everything a declaration cannot rebuild is refused with the
//! node's IRI and why, never skipped. The Turtle half (`Topology::from_turtle`) runs
//! under the `declare` feature, which CI's `features: "*"` job enables.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    build, Alias, AliasTable, AsyncFnEndpoint, Capability, Confine, DeclarationError, Endpoint,
    EndpointSpace, Exact, Fallback, FnEndpoint, Grammar, Iri, Kernel, Level, Limit, MatchKind,
    Mount, Registry, ReprType, Representation, Request, Resolution, Rewrite, Scope, Space,
    SpaceKind, Topology, UriTemplate, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// An endpoint that answers its own name and the bindings it was resolved with.
fn says(name: &'static str) -> Arc<dyn Endpoint> {
    Arc::new(FnEndpoint::new(name, move |inv| {
        let bindings: Vec<String> = inv
            .bindings
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        Ok(Representation::new(
            ReprType::new("text/plain"),
            format!("{name} {}", bindings.join(",")).into_bytes(),
        )
        .cacheable())
    }))
}

/// An endpoint that answers by reading `target` — so where its sub-request resolves
/// (a level, a confined corridor) is part of what it answers.
fn reads(name: &'static str, target: &'static str) -> Arc<dyn Endpoint> {
    Arc::new(AsyncFnEndpoint::new(name, move |inv| {
        Box::pin(async move { inv.source(&Iri::parse(target).unwrap()).await })
    }))
}

/// A registry holding every endpoint given, each once.
fn registry(endpoints: &[&Arc<dyn Endpoint>]) -> Registry {
    let mut registry = Registry::new();
    for endpoint in endpoints {
        registry.register(Arc::clone(endpoint)).unwrap();
    }
    registry
}

// ---- the fixtures ------------------------------------------------------------

/// Every rebuildable kind, named and anonymous: three limiters (prefix, exact,
/// template), a mount over a sealing level, an alias with a hop bound over a leaf with
/// a shadowed door and two confined doors, and a leaf of templates.
fn every_kind() -> (Arc<dyn Space>, Registry) {
    let public = reads("mod-public", "urn:test:internal:helper");
    let helper = says("mod-helper");
    let thing = says("thing");
    let open = says("open-any");
    let shadowed = says("shadowed");
    let extract = reads("extract", "urn:test:ctx:doc:body");
    let leak = reads("leak", "urn:test:open:thing");
    let body = says("body");
    let private = says("private");
    let doc = says("doc");
    let registry = registry(&[
        &public, &helper, &thing, &open, &shadowed, &extract, &leak, &body, &private, &doc,
    ]);

    let corridor = || -> Arc<dyn Space> {
        Arc::new(Fallback::new(vec![Arc::new(
            EndpointSpace::new().bind_arc(Exact::new("urn:test:ctx:doc:body"), Arc::clone(&body)),
        )]))
    };
    let module = EndpointSpace::new()
        .bind_arc(Exact::new("urn:test:mod:public"), Arc::clone(&public))
        .bind_arc(Exact::new("urn:test:internal:helper"), Arc::clone(&helper));
    let table = AliasTable::new()
        .prefix("urn:test:old:", "urn:test:open:")
        .exact("urn:test:short", "urn:test:open:thing")
        .with_max_hops(3);
    let open_space = EndpointSpace::new()
        .bind_arc(Exact::new("urn:test:open:thing"), Arc::clone(&thing))
        .bind_arc(
            UriTemplate::parse("urn:test:open:{name}").unwrap(),
            Arc::clone(&open),
        )
        // Never reached: the first door wins. Order is meaning.
        .bind_arc(Exact::new("urn:test:open:thing"), Arc::clone(&shadowed))
        .bind_arc(
            Exact::new("urn:test:extract"),
            Arc::new(Confine::new(
                iri("urn:test:ctx:doc"),
                corridor(),
                Arc::clone(&extract),
            )),
        )
        .bind_arc(
            Exact::new("urn:test:leak"),
            Arc::new(Confine::new(
                iri("urn:test:ctx:doc"),
                corridor(),
                Arc::clone(&leak),
            )),
        )
        .named(iri("urn:test:space:open"));
    let root: Arc<dyn Space> = Arc::new(
        Fallback::new(vec![
            Arc::new(Limit::new("urn:test:secret:")),
            Arc::new(
                Limit::matching(Exact::new("urn:test:open:hidden"))
                    .named(iri("urn:test:space:hide")),
            ),
            Arc::new(Limit::matching(
                UriTemplate::parse("urn:test:doc:{id}:private").unwrap(),
            )),
            Arc::new(Mount::new(
                "urn:test:mod:",
                Arc::new(
                    Level::new(iri("urn:test:level:mod"), Arc::new(module))
                        .sealing(["urn:test:mod:sealed:"]),
                ),
            )),
            Arc::new(
                Alias::new(Arc::new(table), Arc::new(open_space))
                    .named(iri("urn:test:space:alias")),
            ),
            Arc::new(
                EndpointSpace::new()
                    .bind_arc(
                        UriTemplate::parse("urn:test:doc:{id}:private").unwrap(),
                        Arc::clone(&private),
                    )
                    .bind_arc(
                        UriTemplate::parse("urn:test:doc:{id}").unwrap(),
                        Arc::clone(&doc),
                    ),
            ),
        ])
        .named(iri("urn:test:space:root")),
    );
    (root, registry)
}

/// The shape `crates/tic-tac-toe` in ikigai-tutorial composes, with `urn:test:` names:
/// fourteen doors (thirteen composites and the stored cell), and eight exact aliases
/// that name the lines of the board onto the `cells:{list}` template, over a fallback.
fn tic_tac_toe() -> (Arc<dyn Space>, Registry) {
    const T: &str = "urn:test:ttt:";
    let doors: [(&str, &'static str); 13] = [
        ("cell:{x}:{y}", "ttt-cell"),
        ("cells:{list}", "ttt-cells"),
        ("board", "ttt-board"),
        ("checkset:{x}:{y}", "ttt-checkset"),
        ("winner", "ttt-winner"),
        ("turn", "ttt-turn"),
        ("move:{x}:{y}", "ttt-move"),
        ("reset", "ttt-reset"),
        ("template:{name}", "ttt-template"),
        ("view:board", "ttt-view-board"),
        ("view:square:{x}:{y}", "ttt-view-square"),
        ("view:status", "ttt-view-status"),
        ("view:game:{game}", "ttt-view-game"),
    ];
    let mut endpoints: Vec<Arc<dyn Endpoint>> = Vec::new();
    let mut composites = EndpointSpace::new();
    for (tail, name) in doors {
        let endpoint = says(name);
        let pattern = format!("{T}{tail}");
        composites = if tail.contains('{') {
            composites.bind_arc(UriTemplate::parse(pattern).unwrap(), Arc::clone(&endpoint))
        } else {
            composites.bind_arc(Exact::new(pattern), Arc::clone(&endpoint))
        };
        endpoints.push(endpoint);
    }
    let stored = says("ttt-stored");
    endpoints.push(Arc::clone(&stored));
    let store = EndpointSpace::new().bind_arc(
        UriTemplate::parse(format!("{T}stored:{{x}}:{{y}}")).unwrap(),
        stored,
    );
    let lines = [
        ("row:0", "0.0,1.0,2.0"),
        ("row:1", "0.1,1.1,2.1"),
        ("row:2", "0.2,1.2,2.2"),
        ("column:0", "0.0,0.1,0.2"),
        ("column:1", "1.0,1.1,1.2"),
        ("column:2", "2.0,2.1,2.2"),
        ("diagonal:0", "0.0,1.1,2.2"),
        ("diagonal:1", "2.0,1.1,0.2"),
    ];
    let mut table = AliasTable::new();
    for (line, cells) in lines {
        table = table.exact(format!("{T}{line}"), format!("{T}cells:{cells}"));
    }
    let root: Arc<dyn Space> = Arc::new(Alias::new(
        Arc::new(table),
        Arc::new(Fallback::new(vec![Arc::new(composites), Arc::new(store)])),
    ));
    let refs: Vec<&Arc<dyn Endpoint>> = endpoints.iter().collect();
    (root, registry(&refs))
}

// ---- the round trip ------------------------------------------------------------

/// A template's instance: every `{var}` filled with `v`.
fn instance(pattern: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        out.push('v');
        rest = &rest[open..];
        rest = &rest[rest.find('}').map_or(rest.len(), |close| close + 1)..];
    }
    out.push_str(rest);
    out
}

/// Names to ask both kernels, drawn from the arrangement itself.
fn samples(tree: &Topology, out: &mut BTreeSet<String>) {
    match &tree.kind {
        SpaceKind::EndpointSpace { doors } => {
            for door in doors {
                out.insert(instance(&door.pattern));
                out.insert(format!("{}-miss", instance(&door.pattern)));
                if let Some(corridor) = &door.confined {
                    samples(corridor, out);
                }
            }
        }
        SpaceKind::Alias { rules, .. } => {
            for rule in rules {
                out.insert(rule.from.clone());
                out.insert(format!("{}tail", rule.from));
            }
        }
        SpaceKind::Limit { family, .. } => {
            out.insert(instance(family));
            out.insert(format!("{}x", instance(family)));
        }
        SpaceKind::Mount { prefix } => {
            out.insert(format!("{prefix}nothing"));
        }
        SpaceKind::Level { seals, .. } => {
            for seal in seals {
                out.insert(format!("{seal}x"));
            }
        }
        _ => {}
    }
    for child in &tree.children {
        samples(child, out);
    }
}

/// What a space resolves a name to, in the terms a caller can see.
fn resolved(space: &Arc<dyn Space>, name: &str) -> Option<String> {
    let request = Request::new(Verb::Source, iri(name));
    match space.resolve(&request, &Scope::empty()) {
        Resolution::Miss => None,
        Resolution::Hit(hit) => {
            let bindings: Vec<String> = hit
                .bindings
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            Some(format!(
                "endpoint={} bindings=[{}] answered_by={:?} canonical={:?} levels={} limiter={}",
                hit.endpoint.name(),
                bindings.join(","),
                hit.answered_by.as_ref().map(Iri::as_str),
                hit.canonical.as_ref().map(Iri::as_str),
                hit.levels(),
                hit.endpoint.is_limiter(),
            ))
        }
    }
}

/// What a kernel answers: the bytes, or the refusal as text.
fn answered(kernel: &Kernel, name: &str) -> Result<Vec<u8>, String> {
    block_on(kernel.issue(Request::new(Verb::Source, iri(name)), &Capability::root()))
        .map(|rep| rep.bytes)
        .map_err(|e| e.to_string())
}

/// Build K′ from K's arrangement and hold it to K: a fixpoint, and the same answers.
/// Returns how many names were compared.
fn round_trip(original: Arc<dyn Space>, registry: &Registry, rebuilt: Arc<dyn Space>) -> usize {
    assert_eq!(rebuilt.topology(), original.topology(), "not a fixpoint");
    assert_eq!(
        rebuilt.topology().to_turtle(),
        original.topology().to_turtle(),
        "not a fixpoint as Turtle"
    );
    let mut names = BTreeSet::new();
    samples(&original.topology(), &mut names);
    names.insert("urn:test:nowhere".to_string());
    let k = Kernel::new(Arc::clone(&original)).with_sealed(registry.sealed().iter().cloned());
    let k2 = Kernel::new(Arc::clone(&rebuilt)).with_sealed(registry.sealed().iter().cloned());
    for name in &names {
        assert_eq!(
            resolved(&rebuilt, name),
            resolved(&original, name),
            "{name} resolves differently"
        );
        assert_eq!(
            answered(&k2, name),
            answered(&k, name),
            "{name} is answered differently"
        );
    }
    names.len()
}

#[test]
fn every_rebuildable_kind_round_trips_to_a_fixpoint_that_answers_every_name_the_same() {
    let (original, registry) = every_kind();
    let rebuilt = build(&original.topology(), &registry).unwrap();
    let compared = round_trip(Arc::clone(&original), &registry, rebuilt);
    assert!(compared >= 25, "compared only {compared} names");

    // The answers the fixture exists to exercise, stated once so the round trip
    // cannot pass by both kernels failing alike.
    let k = Kernel::new(original);
    let text = |name: &str| String::from_utf8(answered(&k, name).unwrap()).unwrap();
    assert_eq!(text("urn:test:open:thing"), "thing ", "the first door wins");
    assert_eq!(text("urn:test:short"), "thing ", "an exact alias");
    assert_eq!(text("urn:test:old:x"), "open-any name=x", "a prefix alias");
    assert_eq!(
        text("urn:test:mod:public"),
        "mod-helper ",
        "a level's sub-request"
    );
    assert_eq!(text("urn:test:extract"), "body ", "a confined sub-request");
    assert!(
        answered(&k, "urn:test:leak").is_err(),
        "the root is cut off"
    );
    assert!(
        answered(&k, "urn:test:open:hidden").is_err(),
        "an exact limiter"
    );
    assert!(
        answered(&k, "urn:test:doc:v:private").is_err(),
        "a template limiter"
    );
    assert_eq!(text("urn:test:doc:v"), "doc id=v");
    assert!(
        answered(&k, "urn:test:secret:x").is_err(),
        "a prefix limiter"
    );
}

#[test]
fn a_tic_tac_toe_shaped_space_round_trips() {
    let (original, registry) = tic_tac_toe();
    let tree = original.topology();
    let doors: usize = {
        fn count(t: &Topology) -> usize {
            let here = match &t.kind {
                SpaceKind::EndpointSpace { doors } => doors.len(),
                _ => 0,
            };
            here + t.children.iter().map(count).sum::<usize>()
        }
        count(&tree)
    };
    assert_eq!(doors, 14);
    assert!(matches!(&tree.kind, SpaceKind::Alias { rules, .. } if rules.len() == 8));
    assert_eq!(registry.len(), 14);

    let rebuilt = build(&tree, &registry).unwrap();
    round_trip(Arc::clone(&original), &registry, Arc::clone(&rebuilt));
    // A line is a name for a list of cells, answered by the one template.
    let k = Kernel::new(rebuilt);
    assert_eq!(
        answered(&k, "urn:test:ttt:diagonal:1").unwrap(),
        b"ttt-cells list=2.0,1.1,0.2"
    );
}

// ---- the refusals ----------------------------------------------------------------

/// The refusal a build gave (a built space has no `Debug`, so no `unwrap_err`).
fn refusal(built: Result<Arc<dyn Space>, DeclarationError>) -> DeclarationError {
    match built {
        Ok(_) => panic!("built, and should have been refused"),
        Err(e) => e,
    }
}

/// A space that says nothing about itself: the default topology.
struct Foreign;
impl Space for Foreign {
    fn resolve(&self, _: &Request, _: &Scope) -> Resolution {
        Resolution::Miss
    }
}

/// A grammar written outside core.
struct Tagged;
impl Grammar for Tagged {
    fn match_iri(&self, iri: &Iri) -> Option<ikigai_core::Bindings> {
        (iri.as_str() == "urn:test:tagged").then(ikigai_core::Bindings::new)
    }

    fn pattern(&self) -> String {
        "urn:test:tagged".into()
    }
}

#[test]
fn what_is_not_data_is_refused_by_name_never_skipped() {
    let x = says("x");
    let registry = registry(&[&x]);
    let refuse = |space: Arc<dyn Space>| refusal(build(&space.topology(), &registry));

    // A remote peer's shape — an opaque space under a mount — points at ledger #630.
    let err = refuse(Arc::new(Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:test:x"), Arc::clone(&x))),
        Arc::new(Mount::new("urn:test:peer:", Arc::new(Foreign))),
    ])));
    assert_eq!(
        err,
        DeclarationError::Opaque {
            node: "urn:ikigai:space:_:4".into(),
            mounted_at: Some("urn:test:peer:".into())
        }
    );
    assert!(err.to_string().contains("ledger #630"), "{err}");
    // … and one anywhere else says what it is, without the pointer.
    let err = refuse(Arc::new(Fallback::new(vec![Arc::new(Foreign)])));
    assert!(
        matches!(
            err,
            DeclarationError::Opaque {
                mounted_at: None,
                ..
            }
        ),
        "{err}"
    );

    // A closure rewrite: the node's own name, and why.
    let err = refuse(Arc::new(
        Rewrite::new(Arc::new(EndpointSpace::new()), |_| None).named(iri("urn:test:space:tau")),
    ));
    assert_eq!(
        err,
        DeclarationError::Rewrite {
            node: "urn:test:space:tau".into()
        }
    );
    assert!(err.to_string().contains("closure"), "{err}");

    // A chain given as a root: what `urn:kernel:topology` answers is per request.
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let err = refusal(build(&kernel.topology(), &registry));
    assert_eq!(
        err,
        DeclarationError::Chain {
            node: "urn:ikigai:chain:root".into()
        }
    );

    // A confinement where a space belongs.
    let confine = Confine::new(
        iri("urn:test:ctx:c"),
        Arc::new(EndpointSpace::new()),
        Arc::clone(&x),
    );
    let err = refusal(build(&confine.topology(), &registry));
    assert_eq!(
        err,
        DeclarationError::Confine {
            node: "urn:test:ctx:c".into()
        }
    );

    // A custom grammar is code: its pattern describes it and does not define it.
    let err = refuse(Arc::new(
        EndpointSpace::new()
            .bind_arc(Tagged, Arc::clone(&x))
            .named(iri("urn:test:space:t")),
    ));
    assert!(
        matches!(&err, DeclarationError::Pattern { node, .. } if node == "urn:test:space:t:door:1"),
        "{err:?}"
    );

    // An endpoint the host did not register: the door, and the name it binds.
    let err = refuse(Arc::new(
        EndpointSpace::new()
            .bind_arc(Exact::new("urn:test:x"), Arc::clone(&x))
            .bind_arc(Exact::new("urn:test:y"), says("y")),
    ));
    assert_eq!(
        err,
        DeclarationError::UnknownEndpoint {
            door: "urn:ikigai:space:_:1:door:2".into(),
            id: "y".into()
        }
    );
    assert!(err.to_string().contains("never mints"), "{err}");
}

#[test]
fn a_name_met_twice_is_one_space_and_two_arrangements_under_one_name_are_refused() {
    let (x, y) = (says("x"), says("y"));
    let registry = registry(&[&x, &y]);
    let leaf = |endpoint: &Arc<dyn Endpoint>| -> Arc<dyn Space> {
        Arc::new(
            EndpointSpace::new()
                .bind_arc(Exact::new("urn:test:x"), Arc::clone(endpoint))
                .named(iri("urn:test:space:shared")),
        )
    };
    // The same arrangement under one name, twice: built once, shared.
    let twice = Fallback::new(vec![
        Arc::new(Mount::new("urn:test:a:", leaf(&x))),
        Arc::new(Mount::new("urn:test:", leaf(&x))),
    ]);
    assert!(build(&twice.topology(), &registry).is_ok());
    // Two different arrangements claiming one name: refused, never built as two.
    let clash = Fallback::new(vec![leaf(&x), leaf(&y)]);
    let err = refusal(build(&clash.topology(), &registry));
    assert!(
        matches!(&err, DeclarationError::Malformed { node: Some(n), .. } if n == "urn:test:space:shared"),
        "{err:?}"
    );

    // And never RENDERED as one (ledger #644): `to_turtle` writes a named node once,
    // where it is first met, so the clash would read back as the first arrangement
    // alone — a tree `build` accepts where the original is refused. The checked
    // render refuses with build's own refusal, and so does `urn:kernel:topology`.
    #[cfg(feature = "declare")]
    {
        let lossy = Topology::from_turtle(&clash.topology().to_turtle()).unwrap();
        assert!(build(&lossy, &registry).is_ok(), "the lossy render builds");
    }
    assert_eq!(clash.topology().try_to_turtle().unwrap_err(), err);
    assert_eq!(
        twice.topology().try_to_turtle().unwrap(),
        twice.topology().to_turtle()
    );
    let kernel = Kernel::new(Arc::new(clash));
    let answer = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:kernel:topology")),
        &Capability::root(),
    ));
    match answer {
        Err(ikigai_core::Error::Conflict(message)) => {
            assert!(message.contains("urn:test:space:shared"), "{message}")
        }
        other => panic!("rendered a clash: {:?}", other.map(|r| r.bytes)),
    }
}

#[test]
fn two_endpoints_sharing_a_name_cannot_both_be_registered() {
    // The name ambiguity the topology cannot see: K binds two DIFFERENT endpoints
    // that are both called `x`, and renders two doors naming `x`. The registry is
    // where that is refused, so no door can bind "whichever x came first".
    let (a, b) = (says("x"), says("x"));
    let k = EndpointSpace::new()
        .bind_arc(Exact::new("urn:test:a"), Arc::clone(&a))
        .bind_arc(Exact::new("urn:test:b"), Arc::clone(&b));
    assert!(
        matches!(&k.topology().kind, SpaceKind::EndpointSpace { doors }
        if doors.iter().all(|d| d.endpoint == "x"))
    );
    let mut registry = Registry::new();
    registry.register(a).unwrap();
    assert_eq!(
        registry.register(b).unwrap_err(),
        DeclarationError::DuplicateEndpoint { id: "x".into() }
    );
}

#[test]
fn a_declaration_cannot_seal_its_way_into_a_name_the_host_sealed() {
    let x = says("x");
    // A module level that seals the host's namespace, and one that binds a name in it.
    let sealer = Mount::new(
        "urn:test:mod:",
        Arc::new(
            Level::new(iri("urn:test:level:m"), Arc::new(EndpointSpace::new()))
                .sealing(["urn:test:mod:host:"]),
        ),
    );
    let squatter = Mount::new(
        "urn:test:mod:",
        Arc::new(Level::new(
            iri("urn:test:level:m"),
            Arc::new(
                EndpointSpace::new().bind_arc(Exact::new("urn:test:host:key"), Arc::clone(&x)),
            ),
        )),
    );
    let host = registry(&[&x]).sealing(["urn:test:mod:host:", "urn:test:host:"]);
    for space in [sealer.topology(), squatter.topology()] {
        let err = refusal(build(&space, &host));
        assert!(matches!(err, DeclarationError::Sealing(_)), "{err:?}");
    }
    // Without the host's seals both are well-formed arrangements.
    let open = registry(&[&x]);
    assert!(build(&sealer.topology(), &open).is_ok());
    assert!(build(&squatter.topology(), &open).is_ok());
}

#[test]
fn a_prefix_door_and_a_template_that_does_not_parse_are_refused() {
    use ikigai_core::Door;
    let x = says("x");
    let registry = registry(&[&x]);
    let leaf = |door: Door| {
        Topology::new(SpaceKind::EndpointSpace { doors: vec![door] })
            .with_id(Some(iri("urn:test:s")))
    };
    for door in [
        Door::new("urn:test:", MatchKind::Prefix, "x"),
        Door::new("urn:test:{unclosed", MatchKind::Template, "x"),
    ] {
        let err = refusal(build(&leaf(door), &registry));
        assert!(
            matches!(&err, DeclarationError::Pattern { node, .. } if node == "urn:test:s:door:1"),
            "{err:?}"
        );
    }
}

// ---- the Turtle half (the `declare` feature) -------------------------------------

#[cfg(feature = "declare")]
mod turtle {
    use super::*;

    /// Parse, check the parse is the tree, and build.
    fn read_back(original: &Arc<dyn Space>, registry: &Registry) -> Arc<dyn Space> {
        let turtle = original.topology().to_turtle();
        let parsed = Topology::from_turtle(&turtle).unwrap();
        assert_eq!(
            parsed,
            original.topology(),
            "parse is not the inverse of render"
        );
        assert_eq!(parsed.to_turtle(), turtle);
        build(&parsed, registry).unwrap()
    }

    #[test]
    fn every_rebuildable_kind_round_trips_through_turtle() {
        let (original, registry) = every_kind();
        let rebuilt = read_back(&original, &registry);
        round_trip(original, &registry, rebuilt);
    }

    #[test]
    fn a_tic_tac_toe_shaped_space_round_trips_through_turtle() {
        let (original, registry) = tic_tac_toe();
        let rebuilt = read_back(&original, &registry);
        round_trip(original, &registry, rebuilt);
    }

    #[test]
    fn the_whole_chain_parses_and_the_builder_asks_for_its_root_layer() {
        let (original, registry) = every_kind();
        let kernel = Kernel::new(Arc::clone(&original));
        let chain = Topology::from_turtle(&kernel.topology().to_turtle()).unwrap();
        assert_eq!(chain, kernel.topology());
        assert!(matches!(
            build(&chain, &registry),
            Err(DeclarationError::Chain { .. })
        ));
        let root = chain.children.last().unwrap();
        round_trip(original, &registry, build(root, &registry).unwrap());
    }

    /// A small declaration to break in each way `from_turtle` must refuse.
    fn sample() -> String {
        let x = says("x");
        Fallback::new(vec![
            Arc::new(Limit::new("urn:test:secret:")),
            Arc::new(
                EndpointSpace::new()
                    .bind_arc(Exact::new("urn:test:x"), Arc::clone(&x))
                    .bind_arc(UriTemplate::parse("urn:test:t:{v}").unwrap(), x),
            ),
        ])
        .named(iri("urn:test:space:root"))
        .topology()
        .to_turtle()
    }

    fn refused(turtle: &str) -> String {
        match Topology::from_turtle(turtle) {
            Err(DeclarationError::Malformed { node, reason }) => {
                format!("{} {reason}", node.unwrap_or_default())
            }
            other => panic!("not refused as malformed: {other:?}\n{turtle}"),
        }
    }

    #[test]
    fn what_to_turtle_does_not_write_is_refused() {
        let good = sample();
        assert!(Topology::from_turtle(&good).is_ok());
        let cases: Vec<(String, &str)> = vec![
            ("this is not turtle".into(), "not Turtle"),
            // An unknown kind.
            (good.replace("a ik:Limit", "a ik:Hole"), "ik:Hole"),
            // A triple no arrangement writes.
            (
                format!("{good}<urn:test:space:root> <urn:x:note> \"hello\" .\n"),
                "not part of an arrangement",
            ),
            // A blank node.
            (
                good.replace("ik:family \"urn:test:secret:\"", "ik:family [ ]"),
                "blank node",
            ),
            // A list cell with its rest missing.
            (
                good.replace(
                    "rdf:first <urn:ikigai:space:_:1> ;\n    rdf:rest <urn:test:space:root:layer:2> .",
                    "rdf:first <urn:ikigai:space:_:1> .",
                ),
                "rdf:rest is missing",
            ),
            // A cycle through the root: then nothing is a root.
            (
                good.replace(
                    "rdf:first <urn:ikigai:space:_:2> ;\n    rdf:rest rdf:nil",
                    "rdf:first <urn:ikigai:space:_:2> ;\n    rdf:rest <urn:test:loop> .\n\
                     <urn:test:loop> rdf:first <urn:test:space:root> ;\n    rdf:rest rdf:nil",
                ),
                "cycle",
            ),
            // A cycle below the root: a mount that encloses itself.
            (
                good.replace(
                    "rdf:first <urn:ikigai:space:_:2> ;\n    rdf:rest rdf:nil",
                    "rdf:first <urn:ikigai:space:_:2> ;\n    rdf:rest <urn:test:more> .\n\
                     <urn:test:more> rdf:first <urn:test:m> ;\n    rdf:rest rdf:nil .\n\
                     <urn:test:m> a ik:Mount ;\n    ik:prefix \"urn:test:\" ;\n    ik:space <urn:test:m>",
                ),
                "encloses itself",
            ),
            // The flat patterns and the doors disagree.
            (
                good.replacen("ik:pattern \"urn:test:x\" ;", "ik:pattern \"urn:test:z\" ;", 1),
                "disagree",
            ),
            // A kind of match nobody defined.
            (good.replace("\"prefix\"", "\"regex\""), "regex"),
            // Two roots.
            (
                format!("{good}<urn:test:other> a ik:OpaqueSpace .\n"),
                "more than one root",
            ),
            // A doubled property.
            (
                good.replace(
                    "ik:family \"urn:test:secret:\"",
                    "ik:family \"urn:test:secret:\", \"urn:test:other:\"",
                ),
                "stated 2 times",
            ),
        ];
        for (turtle, expected) in cases {
            assert_ne!(
                turtle, good,
                "the case did not change the document: {expected}"
            );
            let reason = refused(&turtle);
            assert!(reason.contains(expected), "{expected:?} not in {reason:?}");
        }
    }

    #[test]
    fn a_hand_written_declaration_builds() {
        // What a surface (s-expressions through ikigai-sexpr, YAML later) would hand
        // core: Turtle in the vocabulary, written by hand, binding registered names.
        let turtle = r#"
@prefix ik: <https://ikigai-rs.dev/ns#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .

<urn:test:space:app> a ik:Fallback ;
    ik:layers <urn:test:space:app:l1> .
<urn:test:space:app:l1> rdf:first <urn:test:space:wall> ; rdf:rest <urn:test:space:app:l2> .
<urn:test:space:app:l2> rdf:first <urn:test:space:doors> ; rdf:rest rdf:nil .

<urn:test:space:wall> a ik:Limit ; ik:family "urn:test:app:admin:" ; ik:matchKind "prefix" .

<urn:test:space:doors> a ik:EndpointSpace ;
    ik:pattern "urn:test:app:hello", "urn:test:app:admin:{op}" ;
    ik:doors <urn:test:space:doors:c1> .
<urn:test:space:doors:c1> rdf:first <urn:test:door:hello> ; rdf:rest <urn:test:space:doors:c2> .
<urn:test:space:doors:c2> rdf:first <urn:test:door:admin> ; rdf:rest rdf:nil .
<urn:test:door:hello> a ik:Door ; ik:pattern "urn:test:app:hello" ;
    ik:matchKind "exact" ; ik:endpointName "hello" .
<urn:test:door:admin> a ik:Door ; ik:pattern "urn:test:app:admin:{op}" ;
    ik:matchKind "template" ; ik:endpointName "admin" .
"#;
        let (hello, admin) = (says("hello"), says("admin"));
        let registry = registry(&[&hello, &admin]);
        let space = build(&Topology::from_turtle(turtle).unwrap(), &registry).unwrap();
        let kernel = Kernel::new(space);
        assert_eq!(answered(&kernel, "urn:test:app:hello").unwrap(), b"hello ");
        // The door is declared, and walled off by the limiter ahead of it.
        assert!(answered(&kernel, "urn:test:app:admin:drop").is_err());
    }

    #[test]
    fn a_hop_bound_of_zero_is_refused() {
        let turtle = Alias::new(Arc::new(AliasTable::new()), Arc::new(EndpointSpace::new()))
            .named(iri("urn:test:space:a"))
            .topology()
            .to_turtle()
            .replace("ik:maxHops 8", "ik:maxHops 0");
        assert!(refused(&turtle).contains("positive"));
    }
}
