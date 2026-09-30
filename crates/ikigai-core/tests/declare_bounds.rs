//! **A declaration past a bound is REFUSED, never a crash** (ledger #643).
//!
//! A declaration is operator input — `ikigai --arrangement` hands its Turtle straight
//! to `Topology::from_turtle` — so the parse and the build hold it to three bounds:
//! nesting depth, nodes once every reference is expanded, and the text those nodes
//! carry. Before these, a chain of about 110 fallbacks overflowed a 2 MiB thread in a
//! debug build (a stack overflow ABORTS the process; nothing can catch it), and a few
//! kilobytes of Turtle whose named nodes each referenced the next several times
//! expanded exponentially.
//!
//! The deep cases run on a spawned thread with a **1 MiB** stack, the smallest the
//! depth bound is measured for, so they prove the stack claim rather than borrowing
//! the test harness's larger one.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    build, Capability, DeclarationBound, DeclarationError, Door, Endpoint, FnEndpoint, Iri, Kernel,
    MatchKind, Registry, ReprType, Representation, Request, Space, SpaceKind, Topology, Verb,
    MAX_DECLARATION_DEPTH, MAX_DECLARATION_NODES, MAX_DECLARATION_TEXT,
};

/// Run `f` on a thread with a 1 MiB stack.
fn on_one_mib<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn registry() -> Registry {
    let x: Arc<dyn Endpoint> = Arc::new(FnEndpoint::new("x", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"x".to_vec(),
        ))
    }));
    let mut registry = Registry::new();
    registry.register(x).unwrap();
    registry
}

/// The refusal a build gave (a built space has no `Debug`, so no `unwrap_err`).
fn refusal(built: Result<Arc<dyn Space>, DeclarationError>) -> DeclarationError {
    match built {
        Ok(_) => panic!("built, and should have been refused"),
        Err(e) => e,
    }
}

/// A leaf with one exact door, `urn:t:x` → `x`.
fn leaf() -> Topology {
    Topology::new(SpaceKind::EndpointSpace {
        doors: vec![Door::new("urn:t:x", MatchKind::Exact, "x")],
    })
}

/// A chain `depth` deep, built WITHOUT recursion (so the test can hold a tree deeper
/// than any recursive walk would survive): `depth - 1` anonymous fallbacks, each over
/// the next, ending in the leaf.
fn fallbacks(depth: usize) -> Topology {
    let mut tree = leaf();
    for _ in 1..depth {
        tree = Topology::new(SpaceKind::Fallback).child(tree);
    }
    tree
}

// ---- the build half (always on) ---------------------------------------------------

#[test]
fn a_tree_at_the_depth_bound_builds_and_one_past_it_is_refused() {
    on_one_mib(|| {
        let registry = registry();
        let space = build(&fallbacks(MAX_DECLARATION_DEPTH), &registry).unwrap();
        let kernel = Kernel::new(space);
        let answer = block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:t:x")),
            &Capability::root(),
        ));
        assert_eq!(answer.unwrap().bytes, b"x");

        let err = refusal(build(&fallbacks(MAX_DECLARATION_DEPTH + 1), &registry));
        assert_eq!(
            err,
            DeclarationError::TooLarge {
                bound: DeclarationBound::Depth,
                limit: MAX_DECLARATION_DEPTH,
                // The first node too deep, named as the Turtle names it: the leaf is
                // the 49th skolem in pre-order.
                node: format!("urn:ikigai:space:_:{}", MAX_DECLARATION_DEPTH + 1),
            }
        );
        assert!(err.to_string().contains("MAX_DECLARATION_DEPTH"), "{err}");
    });
}

#[test]
fn build_measures_before_it_recurses() {
    // 500 levels: past where the build's own recursion would overflow 1 MiB, so a
    // refusal here (rather than an abort) says the depth is measured without
    // recursing. Dropping a tree this deep still fits.
    on_one_mib(|| {
        let err = refusal(build(&fallbacks(500), &registry()));
        assert!(
            matches!(
                err,
                DeclarationError::TooLarge {
                    bound: DeclarationBound::Depth,
                    ..
                }
            ),
            "{err:?}"
        );
    });
}

#[test]
fn a_named_node_met_again_deeper_down_is_measured_where_it_is_met() {
    // A named space 30 deep, used at the top and again 20 levels down. The builder
    // builds a name once and SHARES it, so its own walk never descends the second
    // time — but the space it builds nests 50 deep there, and so would every walk
    // over it. The measure counts every occurrence, and names the shared node.
    let shared = || {
        let mut tree = fallbacks(29);
        tree = Topology::new(SpaceKind::Fallback)
            .child(tree)
            .with_id(Some(iri("urn:t:shared")));
        tree
    };
    let mut deep = shared();
    for _ in 0..20 {
        deep = Topology::new(SpaceKind::Fallback).child(deep);
    }
    let top = Topology::new(SpaceKind::Fallback)
        .child(shared())
        .child(deep);
    let err = refusal(build(&top, &registry()));
    assert!(
        matches!(
            &err,
            DeclarationError::TooLarge { bound: DeclarationBound::Depth, node, .. }
                if node == "urn:t:shared"
        ),
        "{err:?}"
    );
}

#[test]
fn build_counts_nodes_and_text_at_every_occurrence() {
    // One fallback of MAX_DECLARATION_NODES leaves: each leaf is two nodes (the
    // space and its door), so this is well past the node bound.
    let mut wide = Topology::new(SpaceKind::Fallback);
    for _ in 0..MAX_DECLARATION_NODES / 2 {
        wide = wide.child(leaf());
    }
    let err = refusal(build(&wide, &registry()));
    assert!(
        matches!(
            err,
            DeclarationError::TooLarge {
                bound: DeclarationBound::Nodes,
                limit: MAX_DECLARATION_NODES,
                ..
            }
        ),
        "{err:?}"
    );

    // A few nodes, each carrying a large pattern: under the node bound, past the
    // text bound.
    let big = "urn:t:".to_string() + &"x".repeat(1 << 20);
    let mut heavy = Topology::new(SpaceKind::Fallback);
    for _ in 0..(MAX_DECLARATION_TEXT >> 20) + 1 {
        heavy = heavy.child(Topology::new(SpaceKind::EndpointSpace {
            doors: vec![Door::new(big.clone(), MatchKind::Exact, "x")],
        }));
    }
    let err = refusal(build(&heavy, &registry()));
    assert!(
        matches!(
            err,
            DeclarationError::TooLarge {
                bound: DeclarationBound::Text,
                limit: MAX_DECLARATION_TEXT,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("MAX_DECLARATION_TEXT"), "{err}");
}

// ---- the Turtle half (the `declare` feature) --------------------------------------

#[cfg(feature = "declare")]
mod turtle {
    use std::time::{Duration, Instant};

    use super::*;

    const PREFIXES: &str = "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
                            @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n";

    /// A leaf at `me` with the one door `urn:t:x` → `x`, and a confinement to
    /// `corridor` when there is one.
    fn leaf_at(me: &str, corridor: Option<&str>) -> String {
        let confined = corridor.map_or(String::new(), |c| format!(" ; ik:confinedTo <{c}>"));
        format!(
            "<{me}> a ik:EndpointSpace ; ik:pattern \"urn:t:x\" ; ik:doors <{me}:doors:1> .\n\
             <{me}:doors:1> rdf:first <{me}:door:1> ; rdf:rest rdf:nil .\n\
             <{me}:door:1> a ik:Door ; ik:pattern \"urn:t:x\" ; ik:matchKind \"exact\" ; \
             ik:endpointName \"x\"{confined} .\n"
        )
    }

    /// `depth` fallbacks deep, as hand-written Turtle: each over the next, ending in
    /// the leaf.
    fn fallback_chain(depth: usize) -> String {
        let at = |i: usize| format!("urn:ikigai:space:_:{i}");
        let mut t = PREFIXES.to_string();
        for i in 1..depth {
            let (me, next) = (at(i), at(i + 1));
            t += &format!(
                "<{me}> a ik:Fallback ; ik:layers <{me}:layer:1> .\n\
                 <{me}:layer:1> rdf:first <{next}> ; rdf:rest rdf:nil .\n"
            );
        }
        t + &leaf_at(&at(depth), None)
    }

    /// `depth` leaves deep, each door confined to the next — the costliest shape per
    /// level, since a corridor passes through the door as well as the space.
    fn corridor_chain(depth: usize) -> String {
        let at = |i: usize| format!("urn:t:corridor:{i}");
        let mut t = PREFIXES.to_string();
        for i in 1..depth {
            t += &leaf_at(&at(i), Some(&at(i + 1)));
        }
        t + &leaf_at(&at(depth), None)
    }

    #[test]
    fn at_the_depth_bound_the_whole_pipeline_fits_one_mib() {
        on_one_mib(|| {
            let registry = registry();
            for turtle in [
                fallback_chain(MAX_DECLARATION_DEPTH),
                corridor_chain(MAX_DECLARATION_DEPTH),
            ] {
                let tree = Topology::from_turtle(&turtle).unwrap();
                // Every walk a declaration goes through, on the same small stack.
                assert_eq!(tree.clone(), tree);
                assert_eq!(Topology::from_turtle(&tree.to_turtle()).unwrap(), tree);
                let space = build(&tree, &registry).unwrap();
                assert_eq!(space.topology(), tree);
                let kernel = Kernel::new(space);
                let answer = block_on(kernel.issue(
                    Request::new(Verb::Source, iri("urn:t:x")),
                    &Capability::root(),
                ));
                assert_eq!(answer.unwrap().bytes, b"x");
            }
        });
    }

    #[test]
    fn one_past_the_depth_bound_is_refused_by_name_not_a_crash() {
        on_one_mib(|| {
            let err =
                Topology::from_turtle(&fallback_chain(MAX_DECLARATION_DEPTH + 1)).unwrap_err();
            assert_eq!(
                err,
                DeclarationError::TooLarge {
                    bound: DeclarationBound::Depth,
                    limit: MAX_DECLARATION_DEPTH,
                    node: format!("urn:ikigai:space:_:{}", MAX_DECLARATION_DEPTH + 1),
                }
            );
            let err =
                Topology::from_turtle(&corridor_chain(MAX_DECLARATION_DEPTH + 1)).unwrap_err();
            assert_eq!(
                err,
                DeclarationError::TooLarge {
                    bound: DeclarationBound::Depth,
                    limit: MAX_DECLARATION_DEPTH,
                    node: format!("urn:t:corridor:{}", MAX_DECLARATION_DEPTH + 1),
                }
            );
            // Far past it: the parse checks before it descends, so this is the same
            // refusal, not an overflow.
            let err = Topology::from_turtle(&corridor_chain(5_000)).unwrap_err();
            assert!(
                matches!(
                    err,
                    DeclarationError::TooLarge {
                        bound: DeclarationBound::Depth,
                        ..
                    }
                ),
                "{err:?}"
            );
        });
    }

    #[test]
    fn a_billion_laughs_is_refused_fast() {
        // Twenty named fallbacks, each listing the next four times: a few KB of
        // Turtle that expands to 4^20 leaves. It is refused once the expansion passes
        // the node bound, not after it.
        let levels = 20;
        let mut turtle = PREFIXES.to_string();
        for i in 0..levels {
            let me = format!("urn:t:lol:{i}");
            let next = format!("urn:t:lol:{}", i + 1);
            turtle += &format!("<{me}> a ik:Fallback ; ik:layers <{me}:layer:1> .\n");
            for cell in 1..=4 {
                let rest = if cell == 4 {
                    "rdf:nil".to_string()
                } else {
                    format!("<{me}:layer:{}>", cell + 1)
                };
                turtle += &format!("<{me}:layer:{cell}> rdf:first <{next}> ; rdf:rest {rest} .\n");
            }
        }
        turtle += &leaf_at(&format!("urn:t:lol:{levels}"), None);
        assert!(turtle.len() < 8 * 1024, "{} bytes", turtle.len());

        let started = Instant::now();
        let err = Topology::from_turtle(&turtle).unwrap_err();
        let took = started.elapsed();
        assert!(
            matches!(
                err,
                DeclarationError::TooLarge {
                    bound: DeclarationBound::Nodes,
                    limit: MAX_DECLARATION_NODES,
                    ..
                }
            ),
            "{err:?}"
        );
        // Loose enough for a slow CI runner in a debug build; the expansion it stops
        // would not finish at all.
        assert!(took < Duration::from_secs(10), "took {took:?}");
    }

    #[test]
    fn one_large_literal_used_many_times_is_refused_on_text() {
        // A 1 MiB pattern in a named leaf that a fallback lists seventeen times: a
        // few nodes, and 17 MiB of text once expanded.
        let big = "urn:t:".to_string() + &"x".repeat(1 << 20);
        let uses = (MAX_DECLARATION_TEXT >> 20) + 1;
        let mut turtle = PREFIXES.to_string();
        turtle += "<urn:t:top> a ik:Fallback ; ik:layers <urn:t:top:layer:1> .\n";
        for cell in 1..=uses {
            let rest = if cell == uses {
                "rdf:nil".to_string()
            } else {
                format!("<urn:t:top:layer:{}>", cell + 1)
            };
            turtle +=
                &format!("<urn:t:top:layer:{cell}> rdf:first <urn:t:leaf> ; rdf:rest {rest} .\n");
        }
        turtle += &format!(
            "<urn:t:leaf> a ik:EndpointSpace ; ik:pattern \"{big}\" ; ik:doors <urn:t:leaf:doors:1> .\n\
             <urn:t:leaf:doors:1> rdf:first <urn:t:leaf:door:1> ; rdf:rest rdf:nil .\n\
             <urn:t:leaf:door:1> a ik:Door ; ik:pattern \"{big}\" ; ik:matchKind \"exact\" ; \
             ik:endpointName \"x\" .\n"
        );
        let err = Topology::from_turtle(&turtle).unwrap_err();
        assert!(
            matches!(
                &err,
                DeclarationError::TooLarge { bound: DeclarationBound::Text, node, .. }
                    if node == "urn:t:leaf"
            ),
            "{err:?}"
        );
    }
}
