//! **Sub-request delegation for statically enumerated grants** (ledger #3769;
//! `docs/design/sub-request-authority.md`, Part B).
//!
//! The shape that asked for it: a dashboard host serves a resource composed from a fixed
//! file the host names at startup, bound inside a confining corridor. The file door
//! checks `urn:cap:fs:read:<path>` against the sub-request's capability, so a caller
//! holding only the dashboard's own read scope is denied (the reproduction below), and
//! the host's grant (`Kernel::with_grants`) is what lets the dashboard read it
//! (`Invocation::issue_granted`) without its callers holding the file's authority.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, Description, EndpointSpace, Error, Exact, FnEndpoint, GrantError,
    Iri, Kernel, ReprType, Representation, Request, TraceEvent, Tracer, Verb, GRANTED_FOR_NOTE,
    GRANTED_NOTE,
};

const DASH_READ: &str = "urn:cap:dash:read";
const PRICES_READ: &str = "urn:cap:fs:read:/srv/prices.json";
const TRANSCRIPTS_READ: &str = "urn:cap:fs:read:/srv/transcripts";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// A file door the way ikigai-fs gates one: its own check of `urn:cap:fs:read:<path>`
/// against the capability the request arrived with. The answer names the scopes it was
/// computed under, so a test can see whose authority read it. Cacheable, so the cache's
/// keying is exercised.
fn file_door(scope: &'static str, body: &'static str) -> FnEndpoint {
    FnEndpoint::new("file", move |inv| {
        if !inv.capability.allows(scope) {
            return Err(Error::Denied(format!("no grant for {scope}").into()));
        }
        let held = match inv.capability.scopes() {
            None => "root".to_string(),
            Some(held) => held.iter().cloned().collect::<Vec<_>>().join(" "),
        };
        Ok(Representation::new(
            ReprType::new("text/plain"),
            format!("{body} [{held}]").into_bytes(),
        )
        .cacheable())
    })
}

/// The dashboard: confines itself to a corridor holding the host-named price file, and
/// reads it either under its caller's capability (`granted = false`, today's only form)
/// or under the host's delegation (`granted = true`).
fn dashboard(granted: bool) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("dashboard", move |inv| {
        Box::pin(async move {
            let corridor = Arc::new(EndpointSpace::new().bind(
                Exact::new("urn:file:prices"),
                file_door(PRICES_READ, "prices"),
            ));
            let confined = inv.confine(iri("urn:ctx:dash"), corridor);
            let read = source("urn:file:prices");
            let got = if granted {
                confined.issue_granted(read).await?
            } else {
                confined.issue(read).await?
            };
            Ok(got.cacheable())
        })
    })
    .with_description(
        Description::new("dashboard")
            .verb(Verb::Source)
            .requires(DASH_READ),
    )
}

fn caller() -> Capability {
    Capability::root().attenuate([DASH_READ])
}

/// The host's own authority: what it may delegate, and no more.
fn host() -> Capability {
    Capability::root().attenuate([PRICES_READ, TRANSCRIPTS_READ])
}

fn kernel(granted: bool) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:dash:prices"), dashboard(granted)),
    ))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])])
}

#[test]
fn reproduction_a_confined_module_reading_a_host_file_is_denied_under_its_callers_capability() {
    // Today's only form: the sub-request carries the caller's capability unchanged, and
    // the caller holds only the dashboard's own read scope. A grant in the table changes
    // nothing for `issue`: the authority is the caller's.
    let err = block_on(kernel(false).issue(source("urn:dash:prices"), &caller())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
}

#[test]
fn a_delegated_read_succeeds_under_the_grant_and_only_the_grant() {
    let got = block_on(kernel(true).issue(source("urn:dash:prices"), &caller())).unwrap();
    // Read under exactly the grant: the caller's own scope did not come along (replacement,
    // never union).
    assert_eq!(got.bytes, format!("prices [{PRICES_READ}]").into_bytes());
}

#[test]
fn the_floor_stays_the_callers() {
    // A caller without the dashboard's declared scope never reaches it, grant or no grant.
    let stranger = Capability::root().attenuate(["urn:cap:other"]);
    let err = block_on(kernel(true).issue(source("urn:dash:prices"), &stranger)).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
}

#[test]
fn no_grant_is_a_refusal_never_a_fallback_to_the_callers_capability() {
    // Bound outside every grant prefix: even a ROOT caller's authority is not used.
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:ungranted:prices"), dashboard(true)),
    ))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])]);
    let err =
        block_on(kernel.issue(source("urn:ungranted:prices"), &Capability::root())).unwrap_err();
    assert!(
        matches!(&err, Error::Denied(d) if d.to_string().contains("no grant is bound")),
        "{err:?}"
    );
}

#[test]
fn a_module_cannot_reach_a_grant_another_module_was_given() {
    // Module B, ungranted, confines itself to a corridor that binds a name UNDER the
    // dashboard's granted prefix, with its own code there, and asks that code to act
    // under the grant. An endpoint a confinement answered gets no grant.
    let squatter = AsyncFnEndpoint::new("squatter", |inv| {
        Box::pin(async move {
            let stolen = AsyncFnEndpoint::new("stolen", |inv| {
                Box::pin(async move {
                    assert!(inv.grant().is_none(), "a confined binding got a host grant");
                    let corridor = Arc::new(EndpointSpace::new().bind(
                        Exact::new("urn:file:prices"),
                        file_door(PRICES_READ, "prices"),
                    ));
                    inv.confine(iri("urn:ctx:steal"), corridor)
                        .source_granted(&iri("urn:file:prices"))
                        .await
                })
            });
            let corridor = Arc::new(EndpointSpace::new().bind(Exact::new("urn:dash:evil"), stolen));
            inv.confine(iri("urn:ctx:squat"), corridor)
                .source(&iri("urn:dash:evil"))
                .await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:dash:prices"), dashboard(true))
            .bind(Exact::new("urn:mod-b:read"), squatter),
    ))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])]);
    let err = block_on(kernel.issue(source("urn:mod-b:read"), &Capability::root())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
}

#[test]
fn a_grant_is_not_inherited_by_the_endpoint_a_delegated_request_reaches() {
    // The file door the dashboard reads under its grant tries to act under a grant of its
    // own; it was given none, so the dashboard's does not carry down.
    let relay = AsyncFnEndpoint::new("relay", |inv| {
        Box::pin(async move {
            assert!(inv.grant().is_none());
            inv.source_granted(&iri("urn:file:prices")).await
        })
    });
    let reader = AsyncFnEndpoint::new("reader", |inv| {
        Box::pin(async move { inv.source_granted(&iri("urn:relay")).await })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:dash:reader"), reader)
            .bind(Exact::new("urn:relay"), relay)
            .bind(
                Exact::new("urn:file:prices"),
                file_door(PRICES_READ, "prices"),
            ),
    ))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])]);
    let err = block_on(kernel.issue(source("urn:dash:reader"), &caller())).unwrap_err();
    assert!(
        matches!(&err, Error::Denied(d) if d.to_string().contains("`urn:relay`")),
        "{err:?}"
    );
}

#[test]
fn a_delegated_answer_is_never_served_under_a_different_delegation() {
    // Two modules read the SAME file, each under its own grant. The answer the first
    // computed is cached under its grant's fingerprint, so the second, whose grant lacks
    // the file's scope, misses and is refused by the door rather than handed the entry.
    let reader = || {
        AsyncFnEndpoint::new("reader", |inv| {
            Box::pin(async move { inv.source_granted(&iri("urn:file:prices")).await })
        })
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:dash:a"), reader())
            .bind(Exact::new("urn:other:b"), reader())
            .bind(
                Exact::new("urn:file:prices"),
                file_door(PRICES_READ, "prices"),
            ),
    ))
    .with_grants(
        &host(),
        [
            ("urn:dash:", [PRICES_READ]),
            ("urn:other:", [TRANSCRIPTS_READ]),
        ],
    );
    let a = block_on(kernel.issue(source("urn:dash:a"), &Capability::root())).unwrap();
    assert_eq!(a.bytes, format!("prices [{PRICES_READ}]").into_bytes());
    // Twice, so the first module's entry is warm.
    block_on(kernel.issue(source("urn:dash:a"), &Capability::root())).unwrap();
    let err = block_on(kernel.issue(source("urn:other:b"), &Capability::root())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");

    // And a delegated sub-request keys exactly as any capability does: a caller holding
    // the same scope set the grant holds reads the same entry.
    let direct = Capability::root().attenuate([PRICES_READ]);
    let same = block_on(kernel.issue(source("urn:file:prices"), &direct)).unwrap();
    assert_eq!(same.bytes, a.bytes);
}

struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[test]
fn the_trace_names_the_delegation_and_whose_authority_it_served() {
    let tracer = Arc::new(Collect(Mutex::new(Vec::new())));
    block_on(kernel(true).issue_traced(source("urn:dash:prices"), &caller(), tracer.clone()))
        .unwrap();
    let events = tracer.0.lock().unwrap();
    let file = events
        .iter()
        .find(|e| e.target == "urn:file:prices")
        .expect("the delegated read is traced");
    // The effective authority is the grant…
    assert_eq!(file.capability, Some(vec![PRICES_READ.to_string()]));
    // …the delegation is named by the host's prefix…
    assert!(file
        .notes
        .contains(&(GRANTED_NOTE.to_string(), "urn:dash:".to_string())));
    // …and the authority the dashboard was itself invoked under is beside it.
    assert!(file
        .notes
        .contains(&(GRANTED_FOR_NOTE.to_string(), DASH_READ.to_string())));
    // The dashboard's own event ran under its caller's authority and is no delegation.
    let dash = events
        .iter()
        .find(|e| e.target == "urn:dash:prices")
        .unwrap();
    assert_eq!(dash.capability, Some(vec![DASH_READ.to_string()]));
    assert!(!dash.notes.iter().any(|(k, _)| k == GRANTED_NOTE));
}

#[test]
fn the_grant_is_declared_beside_the_binding() {
    let kernel = kernel(true);
    assert_eq!(
        kernel.grants(),
        vec![("urn:dash:".to_string(), vec![PRICES_READ.to_string()])]
    );
    assert_eq!(
        kernel.grant_for(&iri("urn:dash:prices")),
        Some(vec![PRICES_READ.to_string()])
    );
    assert_eq!(kernel.grant_for(&iri("urn:file:prices")), None);
}

#[test]
fn the_manifold_offers_the_dashboard_to_a_caller_holding_only_its_own_scope() {
    // The dashboard declares only its own read scope (no fs scope it would need its
    // callers to hold), and the delegation makes that declaration true: the manifold
    // offers it to such a caller, and the caller's read of it succeeds (the test above).
    let kernel = kernel(true);
    let actions = |cap: &Capability| {
        let answer = block_on(kernel.issue(source("urn:kernel:actions"), cap)).unwrap();
        String::from_utf8(answer.bytes.to_vec()).unwrap()
    };
    let root_view = actions(&Capability::root());
    assert!(root_view.contains("urn:dash:prices"), "{root_view}");
    assert!(!root_view.contains("urn:cap:fs:read"), "{root_view}");
}

#[test]
fn a_host_cannot_delegate_what_it_does_not_hold_and_grants_are_disjoint() {
    let empty = || Kernel::new(Arc::new(EndpointSpace::new()));
    let refused = empty()
        .try_with_grants(&host(), [("urn:dash:", ["urn:cap:fs:read:/etc/shadow"])])
        .err()
        .unwrap();
    assert_eq!(
        refused,
        GrantError::NotHeld {
            prefix: "urn:dash:".into(),
            scope: "urn:cap:fs:read:/etc/shadow".into()
        }
    );
    let refused = empty()
        .try_with_grants(&host(), [("urn:dash:", [PRICES_READ])])
        .unwrap()
        .try_with_grants(&host(), [("urn:dash:prices", [PRICES_READ])])
        .err()
        .unwrap();
    assert!(
        matches!(refused, GrantError::Overlaps { .. }),
        "{refused:?}"
    );
    let refused = empty()
        .try_with_grants(&host(), [("", [PRICES_READ])])
        .err()
        .unwrap();
    assert_eq!(refused, GrantError::EmptyPrefix);
    let refused = empty()
        .try_with_grants(&host(), [("urn:dash:", Vec::<String>::new())])
        .err()
        .unwrap();
    assert!(
        matches!(refused, GrantError::NoScopes { .. }),
        "{refused:?}"
    );
}

/// An endpoint that reads the price file under whatever grant it was given, and says
/// whether it was given one. The thief in the two tests below.
fn thief() -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("thief", |inv| {
        Box::pin(async move { inv.source_granted(&iri("urn:file:prices")).await })
    })
}

#[test]
fn a_level_inside_a_modules_own_corridor_cannot_reach_another_modules_grant() {
    // Round 2 (hub review of PR 333). Module B confines itself to a corridor of its own,
    // and in that corridor is a LEVEL that binds both a relay and a name under the
    // dashboard's granted prefix. The relay is found inside the level, so its
    // sub-requests resolve through the level stack before the confined corridor. A hit
    // from that stack was not counted as confined, so the thief got the dashboard's grant.
    let relay = || {
        AsyncFnEndpoint::new("relay", |inv| {
            Box::pin(async move { inv.source(&iri("urn:dash:stolen")).await })
        })
    };
    let squatter = AsyncFnEndpoint::new("squatter", move |inv| {
        Box::pin(async move {
            let level = ikigai_core::Level::new(
                iri("urn:example:level:mod-b"),
                Arc::new(
                    EndpointSpace::new()
                        .bind(Exact::new("urn:mod-b:relay"), relay())
                        .bind(Exact::new("urn:dash:stolen"), thief()),
                ),
            );
            let corridor = Arc::new(ikigai_core::Fallback::new(vec![
                Arc::new(level) as Arc<dyn ikigai_core::Space>,
                Arc::new(EndpointSpace::new().bind(
                    Exact::new("urn:file:prices"),
                    file_door(PRICES_READ, "prices"),
                )),
            ]));
            inv.confine(iri("urn:ctx:squat"), corridor)
                .source(&iri("urn:mod-b:relay"))
                .await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:dash:prices"), dashboard(true))
            .bind(Exact::new("urn:mod-b:read"), squatter),
    ))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])]);
    let err = block_on(kernel.issue(source("urn:mod-b:read"), &Capability::root())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
}

#[test]
fn a_rewrite_a_space_reports_cannot_move_an_endpoint_into_a_grant() {
    // Round 2 (hub review of PR 333). Module B is mounted at the root as a `Rewrite` over
    // its own doors: it answers `urn:mod-b:read` with its OWN endpoint and reports the
    // backing name as `urn:dash:prices`, under the dashboard's granted prefix. The grant
    // was keyed on that reported name, so B's endpoint acted under the dashboard's grant
    // while the real dashboard still answered `urn:dash:prices`.
    let module_b: Arc<dyn ikigai_core::Space> = Arc::new(ikigai_core::Rewrite::new(
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:dash:prices"), thief())),
        |target| (target.as_str() == "urn:mod-b:read").then(|| iri("urn:dash:prices")),
    ));
    let kernel = Kernel::new(Arc::new(ikigai_core::Fallback::new(vec![
        module_b,
        Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:dash:prices"), dashboard(true))
                .bind(
                    Exact::new("urn:file:prices"),
                    file_door(PRICES_READ, "prices"),
                ),
        ),
    ])))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])]);
    let err = block_on(kernel.issue(source("urn:mod-b:read"), &Capability::root())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
}

#[test]
fn the_hosts_own_routes_into_a_granted_prefix_keep_the_grant() {
    // The two rules above narrow only what a MODULE chose. The host's own tree still
    // delegates: a dashboard found inside a root-level `Level`, and a dashboard reached
    // through the kernel's own alias table (which rewrites before resolution, so the name
    // the kernel asks is already the granted one), both read under the grant.
    let in_level = Kernel::new(Arc::new(ikigai_core::Level::new(
        iri("urn:example:level:dash"),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:dash:prices"), dashboard(true))),
    )))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])]);
    let got = block_on(in_level.issue(source("urn:dash:prices"), &caller())).unwrap();
    assert_eq!(got.bytes, format!("prices [{PRICES_READ}]").into_bytes());

    let aliased = kernel(true).with_aliases(Arc::new(
        ikigai_core::AliasTable::new().exact("urn:pretty:prices", "urn:dash:prices"),
    ));
    let got = block_on(aliased.issue(source("urn:pretty:prices"), &caller())).unwrap();
    assert_eq!(got.bytes, format!("prices [{PRICES_READ}]").into_bytes());
}

/// A root space answering ONE exact name with whatever endpoint it currently holds: the
/// shape of a door a dynamic space's sink swaps in after the kernel was built, and (with
/// `fresh`) of a space that builds a new endpoint per resolution, as a mount's forwarder or
/// a dynamic space's threaded wrapper does.
struct Swappable {
    name: &'static str,
    door: Mutex<Arc<dyn ikigai_core::Endpoint>>,
    fresh: Option<fn() -> AsyncFnEndpoint>,
}

impl ikigai_core::Space for Swappable {
    fn resolve(&self, request: &Request, _scope: &ikigai_core::Scope) -> ikigai_core::Resolution {
        if request.target.as_str() != self.name {
            return ikigai_core::Resolution::Miss;
        }
        let endpoint: Arc<dyn ikigai_core::Endpoint> = match self.fresh {
            Some(build) => Arc::new(build()),
            None => Arc::clone(&self.door.lock().unwrap()),
        };
        ikigai_core::Resolution::Hit(ikigai_core::Resolved::new(
            endpoint,
            ikigai_core::Bindings::new(),
        ))
    }

    fn entries(&self) -> Option<Vec<ikigai_core::SpaceEntry>> {
        Some(vec![ikigai_core::SpaceEntry::new(self.name, "door")])
    }
}

fn reads_under_grant() -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("reader", |inv| {
        Box::pin(async move { inv.source_granted(&iri("urn:file:prices")).await })
    })
}

fn swappable_kernel(space: Arc<Swappable>) -> Kernel {
    Kernel::new(Arc::new(ikigai_core::Fallback::new(vec![
        space as Arc<dyn ikigai_core::Space>,
        Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:file:prices"),
            file_door(PRICES_READ, "prices"),
        )),
    ])))
    .with_grants(&host(), [("urn:dash:", [PRICES_READ])])
}

#[test]
fn a_door_swapped_in_after_construction_gets_no_grant() {
    // Round 3 of the review: the grant belongs to the endpoint the root answered with when
    // the table was built, not to the name. The same name answered later by another
    // endpoint is refused; the original endpoint, swapped back, reads again.
    let original: Arc<dyn ikigai_core::Endpoint> = Arc::new(reads_under_grant());
    let space = Arc::new(Swappable {
        name: "urn:dash:prices",
        door: Mutex::new(Arc::clone(&original)),
        fresh: None,
    });
    let kernel = swappable_kernel(Arc::clone(&space));
    // A distinct caller per step, so the cache (which a real swap would cut) serves none.
    let step = |n: &str| {
        Capability::root().attenuate([DASH_READ.to_string(), format!("urn:cap:step:{n}")])
    };
    let got = block_on(kernel.issue(source("urn:dash:prices"), &step("1"))).unwrap();
    assert_eq!(got.bytes, format!("prices [{PRICES_READ}]").into_bytes());

    *space.door.lock().unwrap() = Arc::new(reads_under_grant());
    let err = block_on(kernel.issue(source("urn:dash:prices"), &step("2"))).unwrap_err();
    assert!(
        matches!(&err, Error::Denied(d) if d.to_string().contains("no grant is bound")),
        "{err:?}"
    );

    *space.door.lock().unwrap() = original;
    assert!(block_on(kernel.issue(source("urn:dash:prices"), &step("3"))).is_ok());
}

#[test]
fn a_binding_without_a_stable_endpoint_identity_is_never_granted() {
    // A space that answers with a fresh endpoint per resolution has no identity to give a
    // grant to, so the binding is refused when the table is built, and its hop fails closed.
    let space = Arc::new(Swappable {
        name: "urn:dash:prices",
        door: Mutex::new(Arc::new(reads_under_grant())),
        fresh: Some(reads_under_grant),
    });
    let kernel = swappable_kernel(space);
    assert_eq!(kernel.grant_for(&iri("urn:dash:prices")), None);
    let err = block_on(kernel.issue(source("urn:dash:prices"), &caller())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
}

#[test]
fn grant_prefixes_match_and_overlap_only_on_a_segment_boundary() {
    // `urn:dash` and `urn:dashboard` name sibling namespaces, so both may be granted…
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:dash:prices"), dashboard(true))
            .bind(Exact::new("urn:dash/raw"), dashboard(true))
            .bind(Exact::new("urn:dashboard:x"), dashboard(true)),
    ))
    .with_grants(
        &host(),
        [
            ("urn:dash", vec![PRICES_READ]),
            ("urn:dashboard", vec![TRANSCRIPTS_READ]),
        ],
    );
    // …and each covers only its own subtree, after a `:` or a `/`.
    assert_eq!(
        kernel.grant_for(&iri("urn:dash:prices")),
        Some(vec![PRICES_READ.to_string()])
    );
    assert_eq!(
        kernel.grant_for(&iri("urn:dash/raw")),
        Some(vec![PRICES_READ.to_string()])
    );
    assert_eq!(
        kernel.grant_for(&iri("urn:dashboard:x")),
        Some(vec![TRANSCRIPTS_READ.to_string()])
    );
    // A prefix that does cover another on a boundary still overlaps it.
    let refused = Kernel::new(Arc::new(EndpointSpace::new()))
        .try_with_grants(
            &host(),
            [
                ("urn:dash", vec![PRICES_READ]),
                ("urn:dash:x", vec![PRICES_READ]),
            ],
        )
        .err()
        .unwrap();
    assert!(
        matches!(refused, GrantError::Overlaps { .. }),
        "{refused:?}"
    );
}
