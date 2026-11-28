//! **Pairwise identifiers for federation: the reproduction and a prototype** (ledger #4513;
//! `docs/design/pairwise-identifiers.md`).
//!
//! The claim the design note starts from: a federated participant is known by one
//! identifier everywhere it goes, so two services that compare notes can join on it. The
//! first three tests pin that on today's kernel, from public API only:
//!
//! - two peers whose doors mint a principal from the same client certificate fingerprint
//!   (the `urn:iki:gonk:client:<fingerprint>` convention in `principal-capability.md`)
//!   hand their endpoints the same string, and partition their caches by it;
//! - a capability carried across a mount serializes its principal verbatim, so the peer
//!   on the far side reads the upstream's name for its caller, although `clamp` throws
//!   that name away and it buys the caller no authority there.
//!
//! The rest prove the note's decisions with a test-only prototype of a per-peer derivation:
//! a stable, distinct name per peer, which the kernel accepts unchanged, which the
//! participant's operator can map back to the one root identity, and which a peer cannot.
//! No `src/` change.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation, Request,
    Verb, MAX_PRINCIPAL_LEN,
};

/// One client certificate's SHA-256 fingerprint, as a door would hex it (made up).
const FINGERPRINT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const READ: &str = "urn:cap:notes:read";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn one_client_iri() -> String {
    format!("urn:iki:gonk:client:{FINGERPRINT}")
}

/// A peer: one kernel whose only endpoint records the principal each request named, and
/// answers cacheably so the cache's partitioning is visible through its hit counter.
struct Peer {
    kernel: Kernel,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

fn peer() -> Peer {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let endpoint = FnEndpoint::new("notes", move |inv| {
        let who = inv.capability.principal().map(str::to_string);
        log.lock().unwrap().push(who.clone());
        Ok(Representation::new(
            ReprType::new("text/plain"),
            format!("notes for {}", who.as_deref().unwrap_or("-")).into_bytes(),
        )
        .cacheable())
    });
    Peer {
        kernel: Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:notes:mine"), endpoint),
        )),
        seen,
    }
}

/// What a peer's door hands the request: its session ceiling, named for the client.
fn session_for(principal: &str) -> Capability {
    Capability::scoped([READ])
        .with_principal(principal)
        .unwrap()
}

fn read(p: &Peer, cap: &Capability) -> Vec<u8> {
    block_on(
        p.kernel
            .issue(Request::new(Verb::Source, iri("urn:notes:mine")), cap),
    )
    .unwrap()
    .bytes
}

// ---------------------------------------------------------------------------------------
// The reproduction: one identifier everywhere.
// ---------------------------------------------------------------------------------------

#[test]
fn reproduction_two_peers_see_the_same_principal_for_one_client() {
    let (ledger, layers) = (peer(), peer());
    let name = one_client_iri();
    read(&ledger, &session_for(&name));
    read(&layers, &session_for(&name));

    let at_ledger = ledger.seen.lock().unwrap().clone();
    let at_layers = layers.seen.lock().unwrap().clone();
    assert_eq!(at_ledger, vec![Some(name.clone())]);
    // The join key: both operators' records hold the same string for the same party.
    assert_eq!(at_ledger, at_layers);
}

#[test]
fn reproduction_a_carried_capability_discloses_the_upstream_principal_on_the_wire() {
    // The upstream's view of its caller: a passkey-authenticated person.
    let upstream_caller = Capability::scoped([READ])
        .with_principal("urn:iki:gonk:passkey:AbC")
        .unwrap();
    // A sub-request to a mounted remote carries the capability in its serialized form.
    let wire = serde_json::to_string(&upstream_caller).unwrap();
    assert!(
        wire.contains("urn:cap:principal:urn:iki:gonk:passkey:AbC"),
        "the carried capability names the upstream's caller: {wire}"
    );

    // And it buys nothing at the far side: the remote's clamp keeps the principal its own
    // channel authenticated (the upstream machine's) and drops the carried one.
    let carried: Capability = serde_json::from_str(&wire).unwrap();
    let remote_ceiling = session_for(&one_client_iri());
    let clamped = remote_ceiling.clamp(&carried);
    assert_eq!(clamped.principal(), Some(one_client_iri().as_str()));
    assert!(clamped.allows(READ));
}

#[test]
fn reproduction_the_carried_name_is_disclosure_only_shedding_it_changes_no_clamp() {
    let remote_ceiling = session_for(&one_client_iri());
    let named = Capability::scoped([READ])
        .with_principal("urn:iki:gonk:passkey:AbC")
        .unwrap();
    // What a door could send instead: the same grants, principal shed by narrowing.
    let shed = named.attenuate([READ]);
    assert_eq!(shed.principal(), None);
    assert!(!serde_json::to_string(&shed).unwrap().contains("passkey"));
    assert_eq!(remote_ceiling.clamp(&named), remote_ceiling.clamp(&shed));
}

// ---------------------------------------------------------------------------------------
// The prototype: a per-peer name derived from one root secret.
// ---------------------------------------------------------------------------------------

/// The domain-separation context of the derivation. Versioned so a later scheme never
/// collides with this one.
const CONTEXT: &str = "ikigai 2026-11-27 pairwise principal v0";

/// The participant's derivation: a keyed BLAKE3 of the peer's identity under a key derived
/// from the root secret. Length-prefixed, so no two peer identities share an input.
fn pairwise(root_secret: &[u8; 32], peer_identity: &str) -> String {
    let key = blake3::derive_key(CONTEXT, root_secret);
    let mut h = blake3::Hasher::new_keyed(&key);
    h.update(&(peer_identity.len() as u64).to_le_bytes());
    h.update(peer_identity.as_bytes());
    format!("urn:iki:pairwise:{}", h.finalize().to_hex())
}

const ROOT: [u8; 32] = [7; 32];
const LEDGER: &str = "urn:iki:host:ledger.example";
const LAYERS: &str = "urn:iki:host:layers.example";

#[test]
fn a_pairwise_name_is_stable_per_peer_and_distinct_across_peers() {
    assert_eq!(pairwise(&ROOT, LEDGER), pairwise(&ROOT, LEDGER));
    assert_ne!(pairwise(&ROOT, LEDGER), pairwise(&ROOT, LAYERS));
    // Another participant at the same peer is another name.
    assert_ne!(pairwise(&ROOT, LEDGER), pairwise(&[8; 32], LEDGER));
    // Length prefixing: concatenation ambiguity cannot make two peers one name.
    assert_ne!(pairwise(&ROOT, "urn:a:bc"), pairwise(&ROOT, "urn:a:b"));
}

#[test]
fn the_kernel_takes_a_pairwise_name_unchanged() {
    let name = pairwise(&ROOT, LEDGER);
    assert!(name.len() <= MAX_PRINCIPAL_LEN);
    let cap = session_for(&name);
    assert_eq!(cap.principal(), Some(name.as_str()));
    assert!(cap.acts_as(&name));
}

#[test]
fn two_peers_see_two_names_and_each_peer_keeps_one_partition() {
    let (ledger, layers) = (peer(), peer());
    let at_ledger = pairwise(&ROOT, LEDGER);
    let at_layers = pairwise(&ROOT, LAYERS);
    // Each peer is visited twice: the second read at each is a hit under the same name.
    for _ in 0..2 {
        read(&ledger, &session_for(&at_ledger));
        read(&layers, &session_for(&at_layers));
    }
    assert_eq!(*ledger.seen.lock().unwrap(), vec![Some(at_ledger.clone())]);
    assert_eq!(*layers.seen.lock().unwrap(), vec![Some(at_layers.clone())]);
    // No shared string: comparing notes does not join them.
    assert_ne!(at_ledger, at_layers);
    assert!(!at_ledger.contains(FINGERPRINT) && !at_layers.contains(FINGERPRINT));
}

#[test]
fn the_operator_links_the_names_and_a_peer_cannot() {
    // The participant's operator holds the root secret and the list of peers it federates
    // with, so it can rebuild the whole table, or invert one name an audit line shows it.
    let peers = [LEDGER, LAYERS];
    let table: BTreeMap<String, &str> = peers.iter().map(|p| (pairwise(&ROOT, p), *p)).collect();
    assert_eq!(table.get(&pairwise(&ROOT, LAYERS)), Some(&LAYERS));

    // A peer that knows the scheme, both peer identities and its own name, but not the
    // root secret, cannot compute the other peer's name: guessing a secret gives another.
    let guessed = pairwise(&[0; 32], LAYERS);
    assert_ne!(guessed, pairwise(&ROOT, LAYERS));
}
