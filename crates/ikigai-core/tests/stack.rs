//! **Stacking one chain onto another** (ledger #582). `Scope::stack` pushes every
//! corridor of an inner chain innermost onto an outer one, under the inner chain's
//! own identities, so two injectors compose — a game corridor and a temporal one, a
//! person's overrides and an instant — without either knowing how the other's chain
//! was built. What these pin: innermost wins in either order, and the two orders are
//! two chains; a chain built by stacking IS the chain built by the same pushes at
//! once (fingerprint, rendering, clock, answers, one shared cache entry); the empty
//! chain on either side changes nothing; confinement and severing carry across as a
//! replay would leave them; a stacked host corridor still stands in for a sealed
//! name; and levels are unaffected.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Exact, FixedClock, FnEndpoint, Iri, Kernel, Level,
    Mount, ReprType, Representation, Request, Scope, Space, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn constant(name: &'static str, body: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(body).cacheable()))
}

/// A space binding one name to one constant.
fn door(target: &'static str, body: &'static [u8]) -> Arc<dyn Space> {
    Arc::new(EndpointSpace::new().bind(Exact::new(target), constant("door", body)))
}

fn get(kernel: &Kernel, target: &str, scope: Scope) -> String {
    match block_on(kernel.issue_in(source(target), &Capability::root(), scope)) {
        Ok(r) => String::from_utf8_lossy(&r.bytes).into_owned(),
        Err(e) => format!("error: {e}"),
    }
}

fn game() -> Scope {
    Scope::empty().with_named(iri("urn:ctx:game:7"), door("urn:cell:a1", b"game's a1"))
}

fn personal() -> Scope {
    Scope::empty().with_named(
        iri("urn:ctx:person:brian"),
        door("urn:cell:a1", b"brian's a1"),
    )
}

fn as_of() -> Scope {
    Scope::empty().with_named_at(
        iri("urn:ctx:time:2026-09-25T18:00Z"),
        door("urn:time:now", b"18:00"),
        Arc::new(FixedClock::at(1_000)),
    )
}

/// An empty root: every name here is answered by a corridor or not at all.
fn kernel() -> Kernel {
    Kernel::new(Arc::new(EndpointSpace::new()))
}

#[test]
fn two_corridors_stacked_in_either_order_innermost_wins_and_the_fingerprints_differ() {
    let kernel = kernel();
    // The personal corridor stacked onto the game: brian's override wins.
    let over = game().stack(&personal());
    // The game stacked onto the personal corridor: the game's wins.
    let under = personal().stack(&game());
    assert_eq!(get(&kernel, "urn:cell:a1", over.clone()), "brian's a1");
    assert_eq!(get(&kernel, "urn:cell:a1", under.clone()), "game's a1");
    assert_ne!(over.fingerprint(), under.fingerprint());
    assert_eq!(over.to_string(), "urn:ctx:person:brian urn:ctx:game:7 root");
    assert_eq!(
        under.to_string(),
        "urn:ctx:game:7 urn:ctx:person:brian root"
    );
    // Two chains, two cache entries: neither order is served the other's answer.
    assert_eq!(kernel.cache_len(), 2);
    // And a stacked chain is a different chain from either half.
    assert_ne!(over.fingerprint(), game().fingerprint());
    assert_ne!(over.fingerprint(), personal().fingerprint());
}

#[test]
fn a_chain_built_by_stacking_is_the_chain_built_by_the_same_pushes_at_once() {
    let kernel = kernel();
    let stacked = game().stack(&personal()).stack(&as_of());
    let at_once = Scope::empty()
        .with_named(iri("urn:ctx:game:7"), door("urn:cell:a1", b"game's a1"))
        .with_named(
            iri("urn:ctx:person:brian"),
            door("urn:cell:a1", b"brian's a1"),
        )
        .with_named_at(
            iri("urn:ctx:time:2026-09-25T18:00Z"),
            door("urn:time:now", b"18:00"),
            Arc::new(FixedClock::at(1_000)),
        );
    assert_eq!(stacked.fingerprint(), at_once.fingerprint());
    assert_eq!(stacked.to_string(), at_once.to_string());
    assert_eq!(stacked.spaces().len(), 3);
    assert_eq!(stacked.is_severed(), at_once.is_severed());
    assert_eq!(
        stacked.now().map(|t| t.as_millis()),
        at_once.now().map(|t| t.as_millis())
    );
    // Stacking is associative: (game ∘ personal) ∘ as-of = game ∘ (personal ∘ as-of).
    let grouped = game().stack(&personal().stack(&as_of()));
    assert_eq!(grouped.fingerprint(), stacked.fingerprint());
    // Same chain, same answers, ONE cache entry per name between them.
    assert_eq!(get(&kernel, "urn:cell:a1", stacked.clone()), "brian's a1");
    assert_eq!(get(&kernel, "urn:time:now", stacked.clone()), "18:00");
    assert_eq!(get(&kernel, "urn:cell:a1", at_once.clone()), "brian's a1");
    assert_eq!(get(&kernel, "urn:time:now", at_once), "18:00");
    assert_eq!(kernel.cache_len(), 2);
    assert!(kernel.is_cached_in(&source("urn:cell:a1"), &Capability::root(), &grouped));
}

#[test]
fn stacking_the_empty_chain_on_either_side_changes_nothing() {
    let game = game();
    assert_eq!(
        game.clone().stack(&Scope::empty()).fingerprint(),
        game.fingerprint()
    );
    assert_eq!(
        Scope::empty().stack(&game).fingerprint(),
        game.fingerprint()
    );
    assert_eq!(Scope::empty().stack(&game).to_string(), game.to_string());
    assert!(Scope::empty().stack(&Scope::empty()).is_empty());
    assert_eq!(Scope::empty().stack(&Scope::empty()).fingerprint(), 0);
    // An anonymous corridor keeps its identity across the stack — a rebuild with
    // `with` could not, because it would mint a new one.
    let anonymous = Scope::empty().with(door("urn:cell:a1", b"anon"));
    assert_eq!(
        Scope::empty().stack(&anonymous).fingerprint(),
        anonymous.fingerprint()
    );
    assert_ne!(
        Scope::empty()
            .with(Arc::clone(&anonymous.spaces()[0]))
            .fingerprint(),
        anonymous.fingerprint()
    );
}

#[test]
fn the_inner_clock_wins_and_an_inner_chain_without_one_keeps_the_outer_clock() {
    let later = Scope::empty().with_named_at(
        iri("urn:ctx:time:later"),
        door("urn:time:now", b"later"),
        Arc::new(FixedClock::at(9_000)),
    );
    assert_eq!(
        as_of().stack(&later).now().map(|t| t.as_millis()),
        Some(9_000)
    );
    assert_eq!(
        as_of().stack(&game()).now().map(|t| t.as_millis()),
        Some(1_000)
    );
    assert_eq!(
        game().stack(&as_of()).now().map(|t| t.as_millis()),
        Some(1_000)
    );
    assert!(game().stack(&personal()).clock().is_none());
}

#[test]
fn severing_and_confinement_carry_across_as_the_replay_would_leave_them() {
    let kernel = Kernel::new(door("urn:root:only", b"root"));
    // Severed if either is.
    let severed = game().stack(&Scope::empty().sever());
    assert!(severed.is_severed());
    assert_eq!(
        get(&kernel, "urn:root:only", severed.clone()),
        "error: no endpoint resolved for urn:root:only"
    );
    assert_eq!(get(&kernel, "urn:root:only", game()), "root");
    assert_eq!(
        severed.fingerprint(),
        game().sever().fingerprint(),
        "stacking a sever is severing"
    );
    // A confined inner chain: its confined corridor goes behind the outer one's,
    // exactly where `confined` would have put it.
    let confined_inner = Scope::empty()
        .with_named(
            iri("urn:ctx:person:brian"),
            door("urn:cell:a1", b"brian's a1"),
        )
        .confined(iri("urn:ctx:sandbox"), door("urn:cell:b2", b"sandbox b2"));
    let stacked = game().stack(&confined_inner);
    let replayed = game()
        .with_named(
            iri("urn:ctx:person:brian"),
            door("urn:cell:a1", b"brian's a1"),
        )
        .confined(iri("urn:ctx:sandbox"), door("urn:cell:b2", b"sandbox b2"));
    assert_eq!(stacked.fingerprint(), replayed.fingerprint());
    assert_eq!(stacked.to_string(), replayed.to_string());
    assert_eq!(
        stacked.to_string(),
        "urn:ctx:person:brian urn:ctx:game:7 urn:ctx:sandbox severed"
    );
    assert_eq!(get(&kernel, "urn:cell:b2", stacked.clone()), "sandbox b2");
    assert_eq!(get(&kernel, "urn:cell:a1", stacked), "brian's a1");
}

/// Sources `targets` in turn and answers with what came back, recording the chain
/// it ran in — so a test can see the level stack an endpoint inside a level runs in.
fn reader(seen: &Arc<Mutex<Vec<String>>>, targets: &'static [&'static str]) -> AsyncFnEndpoint {
    let seen = Arc::clone(seen);
    AsyncFnEndpoint::new("reader", move |inv| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.lock().unwrap().push(inv.scope().to_string());
            let mut out = Vec::new();
            for target in targets {
                let got = inv.source(&iri(target)).await?;
                out.push(String::from_utf8_lossy(&got.bytes).into_owned());
            }
            Ok(text(out.join(" | ").as_bytes()))
        })
    })
}

/// A module mounted at `urn:game:`, inside a level: its reader sources a sibling
/// by its short name and the time.
fn game_module(seen: &Arc<Mutex<Vec<String>>>) -> Kernel {
    Kernel::new(Arc::new(Mount::new(
        "urn:game:",
        Arc::new(Level::new(
            iri("urn:example:level:game"),
            Arc::new(
                EndpointSpace::new()
                    .bind(
                        Exact::new("urn:game:read"),
                        reader(seen, &["urn:cell:a1", "urn:time:now"]),
                    )
                    .bind(Exact::new("urn:cell:a1"), constant("blank", b"empty"))
                    .bind(Exact::new("urn:time:now"), constant("live", b"live")),
            ),
        )),
    )))
}

#[test]
fn stacked_corridors_stand_in_inside_a_level_and_the_level_stack_is_unaffected() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kernel = game_module(&seen);
    // No corridor: the level's own siblings answer, by short name.
    assert_eq!(
        get(&kernel, "urn:game:read", Scope::empty()),
        "empty | live"
    );
    // Stacked: the game's square AND the pinned time, inside the level.
    let both = game().stack(&as_of());
    assert!(
        both.levels().is_empty(),
        "a host-built chain has no level stack"
    );
    assert_eq!(get(&kernel, "urn:game:read", both), "game's a1 | 18:00");
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], "@urn:example:level:game root");
    assert_eq!(
        seen[1],
        "urn:ctx:time:2026-09-25T18:00Z urn:ctx:game:7 @urn:example:level:game root"
    );
}

#[test]
fn stacking_onto_a_resolved_scope_keeps_its_level_stack() {
    // The scope an endpoint inside a level runs in carries that level; stacking a
    // host corridor onto it leaves the stack where it was.
    let captured = Arc::new(Mutex::new(None));
    let grab = {
        let captured = Arc::clone(&captured);
        FnEndpoint::new("grab", move |inv| {
            *captured.lock().unwrap() = Some(inv.scope().clone());
            Ok(text(b"ok"))
        })
    };
    let kernel = Kernel::new(Arc::new(Mount::new(
        "urn:game:",
        Arc::new(Level::new(
            iri("urn:example:level:game"),
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:game:grab"), grab)),
        )),
    )));
    get(&kernel, "urn:game:grab", Scope::empty());
    let resolved = captured.lock().unwrap().clone().unwrap();
    let names = |scope: &Scope| {
        scope
            .levels()
            .names()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&resolved), ["urn:example:level:game"]);
    let stacked = resolved.clone().stack(&as_of());
    assert_eq!(names(&stacked), names(&resolved));
    assert_eq!(
        stacked.to_string(),
        "urn:ctx:time:2026-09-25T18:00Z @urn:example:level:game root"
    );
}

#[test]
fn a_stacked_host_corridor_still_stands_in_for_a_sealed_name() {
    // Host authority survives stacking: a sealed name skips every level but its
    // owner's, and the host's injected corridors may still answer it — a corridor
    // arriving by `stack` is a host corridor like any other.
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:sign:trust-set"), constant("real", b"real")),
    ))
    .with_sealed(["urn:sign:"]);
    let pinned =
        Scope::empty().with_named(iri("urn:ctx:pinned"), door("urn:sign:trust-set", b"pinned"));
    assert_eq!(get(&kernel, "urn:sign:trust-set", game()), "real");
    assert_eq!(
        get(&kernel, "urn:sign:trust-set", game().stack(&pinned)),
        "pinned"
    );
}
