//! **Logical rewrite, end to end.** The acceptance test for the `urn:iki:`
//! namespace migration: with a prefix rewrite installed, every existing name
//! still resolves, and the new name resolves to the same thing.
//!
//! Beside it, one test per forced decision in `alias.rs` — capability ordering
//! (both directions), what `Meta` reports, whether the two names share a cache
//! entry and a golden thread, what the catalog lists, and how chains and cycles
//! terminate.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    builtins, ActionSpec, AliasTable, ArgRef, Capability, Description, EndpointSpace, Exact,
    FnEndpoint, Iri, Kernel, MetaRenderer, ReprType, Representation, Request, Space, TraceEvent,
    Tracer, Verb, ALIAS_MISS_NOTE, ALIAS_NOTE, DENIED_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text() -> ReprType {
    ReprType::new("text/plain").with_param("charset", "utf-8")
}

/// The rewrite the ecosystem actually needs: `urn:fn:` → `urn:iki:fn:`.
fn migration() -> Arc<AliasTable> {
    Arc::new(
        AliasTable::new()
            .prefix("urn:fn:", "urn:iki:fn:")
            .prefix("urn:store:", "urn:iki:store:")
            .prefix("urn:vault:", "urn:iki:vault:"),
    )
}

/// A mutable cell exposed as a resource: `Source` is cacheable and hangs off the
/// golden thread named after the resource, `Sink` replaces the value. The kernel
/// cuts that thread on a successful mutating verb, so a stale `Source` recomputes.
fn cell(name: &'static str, value: Arc<Mutex<String>>) -> FnEndpoint {
    FnEndpoint::new(name, move |inv| {
        let target = inv.request.target.as_str().to_string();
        match inv.request.verb {
            Verb::Sink => {
                let incoming = inv.inline_str("content")?.to_string();
                *value.lock().unwrap() = incoming;
                Ok(Representation::new(text(), b"ok".to_vec()))
            }
            _ => {
                let current = value.lock().unwrap().clone();
                Ok(Representation::new(text(), current.into_bytes())
                    .cacheable()
                    .depends_on(target))
            }
        }
    })
    .with_description(
        Description::new(name)
            .verb(Verb::Source)
            .verb(Verb::Sink)
            .output("text/plain;charset=utf-8"),
    )
}

/// An endpoint that declares a capability requirement, so the floor has something
/// to enforce.
fn guarded() -> FnEndpoint {
    FnEndpoint::new("vaultRead", |_inv| {
        Ok(Representation::new(text(), b"the goods".to_vec()))
    })
    .with_description(
        Description::new("vaultRead")
            .verb(Verb::Source)
            .action(ActionSpec::new(Verb::Source).requires("urn:cap:iki:vault:read")),
    )
}

fn backing_space(cell_value: Arc<Mutex<String>>) -> Arc<dyn Space> {
    Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:iki:fn:toUpper"), builtins::to_upper())
            .bind(Exact::new("urn:iki:store:x"), cell("cellX", cell_value))
            .bind(Exact::new("urn:iki:vault:secret"), guarded()),
    )
}

fn kernel_with_aliases(cell_value: Arc<Mutex<String>>) -> Kernel {
    Kernel::new(backing_space(cell_value)).with_aliases(migration())
}

fn source(kernel: &Kernel, target: &str, cap: &Capability) -> ikigai_core::Result<Vec<u8>> {
    block_on(kernel.issue(Request::new(Verb::Source, iri(target)), cap)).map(|r| r.bytes)
}

// ---------------------------------------------------------------- acceptance

#[test]
fn every_existing_name_still_resolves_and_the_new_name_is_the_same_thing() {
    // THE acceptance test. `urn:fn:toUpper` is bound nowhere — only
    // `urn:iki:fn:toUpper` is — yet both names answer, identically.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let cap = Capability::root();
    let upper = |target: &str| {
        block_on(kernel.issue(
            Request::new(Verb::Source, iri(target)).with_arg("in", ArgRef::Inline(b"hi".to_vec())),
            &cap,
        ))
        .unwrap()
        .bytes
    };
    assert_eq!(upper("urn:fn:toUpper"), b"HI");
    assert_eq!(upper("urn:iki:fn:toUpper"), b"HI");
    // …and they are ONE resource, not two that agree: a single cache entry.
    assert_eq!(kernel.cache_len(), 1);
}

#[test]
fn a_name_outside_the_table_is_untouched() {
    // The transition window must not move anything it was not asked to move.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let err = source(&kernel, "urn:text:wc", &Capability::root()).unwrap_err();
    assert_eq!(err.to_string(), "no endpoint resolved for urn:text:wc");
}

// -------------------------------------------- decision 1: capability ordering

#[test]
fn authority_is_checked_against_the_backing_name_and_a_grant_on_it_holds() {
    // Direction one: the rewrite happens BEFORE the check, so a grant naming the
    // backing resource's declared scope authorizes the aliased request.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let cap = Capability::scoped(["urn:cap:iki:vault:read"]);
    assert_eq!(
        source(&kernel, "urn:vault:secret", &cap).unwrap(),
        b"the goods"
    );
}

#[test]
fn an_alias_can_never_launder_authority() {
    // Direction two, the one that matters: checking the LOGICAL name first would
    // make an alias an escalation device. A capability that grants nothing the
    // backing endpoint declares is refused, whichever name it arrives under.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let cap = Capability::scoped(["urn:cap:vault:read"]); // the PRE-alias spelling
    let err = source(&kernel, "urn:vault:secret", &cap).unwrap_err();
    assert!(
        matches!(err, ikigai_core::Error::Denied(_)),
        "an un-migrated grant must not pass: {err}"
    );
    // …and identically under the new name, so the alias changes nothing about
    // authority in either direction.
    assert!(source(&kernel, "urn:iki:vault:secret", &cap).is_err());
}

#[test]
fn a_denial_through_an_alias_says_so_and_points_at_the_un_migrated_grant() {
    // The silent-failure shape this primitive exists to refuse: a capability that
    // fails by simply not holding, with nothing in any log.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let cap = Capability::scoped(["urn:cap:read:urn:vault:secret"]);
    let message = source(&kernel, "urn:vault:secret", &cap)
        .unwrap_err()
        .to_string();
    assert!(message.contains("urn:cap:iki:vault:read"), "{message}");
    assert!(
        message.contains("urn:vault:secret -> urn:iki:vault:secret"),
        "the denial must name the hop: {message}"
    );
    assert!(
        message.contains("may not have been migrated"),
        "the denial must flag the un-migrated grant: {message}"
    );
}

#[test]
fn a_denial_that_did_not_travel_through_an_alias_is_unchanged() {
    // No alias, no extra noise: the message is exactly what it was before.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let cap = Capability::scoped(["urn:cap:nothing"]);
    let message = source(&kernel, "urn:iki:vault:secret", &cap)
        .unwrap_err()
        .to_string();
    assert_eq!(
        message,
        "denied: capability does not grant `urn:cap:iki:vault:read` \
         (declared by `urn:iki:vault:secret`)"
    );
}

// ------------------------------------------------- decision 2: what Meta says

struct IdRenderer;

impl MetaRenderer for IdRenderer {
    fn render(
        &self,
        description: &Description,
        _target: &ReprType,
    ) -> ikigai_core::Result<Representation> {
        Ok(Representation::new(
            text(),
            description.id.clone().into_bytes(),
        ))
    }
}

#[test]
fn meta_reports_the_backing_endpoints_self_description() {
    // Self-description that lied would break the catalog, selection and the MCP
    // projection downstream. Describing `urn:fn:toUpper` describes `toUpper`.
    let kernel = Kernel::with_meta_renderer(
        backing_space(Arc::new(Mutex::new(String::new()))),
        Arc::new(IdRenderer),
    )
    .with_aliases(migration());
    let described = block_on(kernel.issue(
        Request::new(Verb::Meta, iri("urn:fn:toUpper")),
        &Capability::root(),
    ))
    .unwrap();
    assert_eq!(described.bytes, b"toUpper");
}

// ------------------------------- decision 3: one cache entry, one golden thread

#[test]
fn a_sink_through_one_name_invalidates_a_source_through_the_other() {
    // The invalidation bug that is hardest to see, because both answers look
    // plausible. Read the old name (cached), write the NEW name, read the old
    // name again: it must recompute.
    let value = Arc::new(Mutex::new("one".to_string()));
    let kernel = kernel_with_aliases(Arc::clone(&value));
    let cap = Capability::root();

    assert_eq!(source(&kernel, "urn:store:x", &cap).unwrap(), b"one");
    assert_eq!(kernel.cache_len(), 1);

    block_on(
        kernel.issue(
            Request::new(Verb::Sink, iri("urn:iki:store:x"))
                .with_arg("content", ArgRef::Inline(b"two".to_vec())),
            &cap,
        ),
    )
    .unwrap();

    assert_eq!(
        source(&kernel, "urn:store:x", &cap).unwrap(),
        b"two",
        "the logical name served a stale representation after a sink through the backing name"
    );
    // …and the reverse direction: write the OLD name, read the NEW one.
    block_on(
        kernel.issue(
            Request::new(Verb::Sink, iri("urn:store:x"))
                .with_arg("content", ArgRef::Inline(b"three".to_vec())),
            &cap,
        ),
    )
    .unwrap();
    assert_eq!(source(&kernel, "urn:iki:store:x", &cap).unwrap(), b"three");
}

#[test]
fn the_two_names_key_the_same_cache_entry() {
    let kernel = kernel_with_aliases(Arc::new(Mutex::new("one".to_string())));
    let cap = Capability::root();
    source(&kernel, "urn:store:x", &cap).unwrap();
    // The entry is keyed under the BACKING name's request id — the identity a
    // second reader arriving under either name computes.
    assert!(kernel.is_cached(&Request::new(Verb::Source, iri("urn:iki:store:x")), &cap));
    assert!(kernel.is_cached(&Request::new(Verb::Source, iri("urn:store:x")), &cap));
    assert_eq!(kernel.cache_len(), 1);
}

// ---------------------------------------------- decision 4: what is enumerated

#[test]
fn the_catalog_lists_the_backing_name_once() {
    // Over-offering two IRIs for one resource makes an agent choose arbitrarily;
    // listing the alias instead would hide the name the ecosystem is moving to.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let patterns: Vec<String> = kernel
        .entries()
        .expect("enumerable")
        .into_iter()
        .map(|e| e.pattern)
        .collect();
    assert!(patterns.contains(&"urn:iki:fn:toUpper".to_string()));
    assert!(!patterns.iter().any(|p| p == "urn:fn:toUpper"));
}

// ------------------------------------------------ decision 5: chains and loops

#[test]
fn a_chained_rewrite_resolves_through_both_hops() {
    let table = Arc::new(
        AliasTable::new()
            .prefix("urn:old:", "urn:mid:")
            .prefix("urn:mid:", "urn:iki:fn:"),
    );
    let kernel =
        Kernel::new(backing_space(Arc::new(Mutex::new(String::new())))).with_aliases(table);
    let out = block_on(
        kernel.issue(
            Request::new(Verb::Source, iri("urn:old:toUpper"))
                .with_arg("in", ArgRef::Inline(b"hi".to_vec())),
            &Capability::root(),
        ),
    )
    .unwrap();
    assert_eq!(out.bytes, b"HI");
}

#[test]
fn a_cycle_is_refused_rather_than_recursed_or_truncated() {
    let table = Arc::new(
        AliasTable::new()
            .prefix("urn:a:", "urn:b:")
            .prefix("urn:b:", "urn:a:"),
    );
    let kernel =
        Kernel::new(backing_space(Arc::new(Mutex::new(String::new())))).with_aliases(table);
    let err = source(&kernel, "urn:a:thing", &Capability::root()).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("alias cycle"), "{message}");
    assert!(
        message.contains("urn:a:thing -> urn:b:thing -> urn:a:thing"),
        "the refusal must show the loop: {message}"
    );
    assert!(
        !err.is_transient(),
        "an operator must fix this, not retry it"
    );
}

// -------------------------------------------------------------- observability

#[derive(Default)]
struct Collector(Mutex<Vec<TraceEvent>>);

impl Tracer for Collector {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn notes_of(events: &[TraceEvent]) -> Vec<(String, String)> {
    events.iter().flat_map(|e| e.notes.clone()).collect()
}

#[test]
fn a_traced_invocation_through_an_alias_carries_the_hop() {
    let kernel = kernel_with_aliases(Arc::new(Mutex::new("one".to_string())));
    let tracer = Arc::new(Collector::default());
    block_on(kernel.issue_traced(
        Request::new(Verb::Source, iri("urn:store:x")),
        &Capability::root(),
        Arc::clone(&tracer) as Arc<dyn Tracer>,
    ))
    .unwrap();
    let events = tracer.0.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    // The event's target is the canonical name — identity is canonical — and the
    // note is what discloses where it came from.
    assert_eq!(events[0].target, "urn:iki:store:x");
    assert!(notes_of(&events).contains(&(
        ALIAS_NOTE.to_string(),
        "urn:store:x -> urn:iki:store:x".to_string()
    )));
}

#[test]
fn a_denial_through_an_alias_is_traced_as_both_facts() {
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let tracer = Arc::new(Collector::default());
    let _ = block_on(kernel.issue_traced(
        Request::new(Verb::Source, iri("urn:vault:secret")),
        &Capability::scoped(["urn:cap:nothing"]),
        Arc::clone(&tracer) as Arc<dyn Tracer>,
    ));
    let notes = notes_of(&tracer.0.lock().unwrap());
    assert!(notes.iter().any(|(key, _)| key == DENIED_NOTE), "{notes:?}");
    assert!(notes.iter().any(
        |(key, value)| key == ALIAS_NOTE && value == "urn:vault:secret -> urn:iki:vault:secret"
    ));
}

#[test]
fn a_miss_after_a_rewrite_is_reported_as_a_rewrite() {
    // "It resolves under the old name but not the new one" — with the alias named.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let tracer = Arc::new(Collector::default());
    let err = block_on(kernel.issue_traced(
        Request::new(Verb::Source, iri("urn:fn:missing")),
        &Capability::root(),
        Arc::clone(&tracer) as Arc<dyn Tracer>,
    ))
    .unwrap_err();
    // The error names the canonical target: the caller knows what they typed, and
    // the name they have never seen is the informative half.
    assert_eq!(
        err.to_string(),
        "no endpoint resolved for urn:iki:fn:missing"
    );
    let notes = notes_of(&tracer.0.lock().unwrap());
    assert!(notes
        .iter()
        .any(|(key, value)| key == ALIAS_NOTE && value == "urn:fn:missing -> urn:iki:fn:missing"));
    assert!(notes
        .iter()
        .any(|(key, value)| key == ALIAS_MISS_NOTE && value == "urn:iki:fn:missing"));
}

#[test]
fn the_live_table_is_readable_as_a_resource_with_its_counters() {
    // The always-on channel: no tracer installed, and the operator can still see
    // which rule fired and which rule fired onto nothing.
    let kernel = kernel_with_aliases(Arc::new(Mutex::new("one".to_string())));
    let cap = Capability::root();
    source(&kernel, "urn:store:x", &cap).unwrap();
    let _ = source(&kernel, "urn:fn:missing", &cap);

    let readout = String::from_utf8(source(&kernel, "urn:kernel:aliases", &cap).unwrap()).unwrap();
    assert!(readout.contains("urn:fn:"), "{readout}");
    assert!(readout.contains("-> "), "{readout}");
    assert!(
        readout.contains("1 hops, 1 unresolved"),
        "the miss must be attributed to the rule that moved the name: {readout}"
    );
    assert!(readout.contains("max hops  8"), "{readout}");
}

#[test]
fn reading_the_table_needs_the_inspect_capability() {
    let kernel = kernel_with_aliases(Arc::new(Mutex::new(String::new())));
    let denied = source(
        &kernel,
        "urn:kernel:aliases",
        &Capability::scoped(["urn:cap:nothing"]),
    );
    assert!(matches!(denied, Err(ikigai_core::Error::Denied(_))));
}

#[test]
fn a_kernel_with_no_table_says_so_rather_than_looking_empty() {
    let kernel = Kernel::new(backing_space(Arc::new(Mutex::new(String::new()))));
    let readout =
        String::from_utf8(source(&kernel, "urn:kernel:aliases", &Capability::root()).unwrap())
            .unwrap();
    assert!(readout.contains("no rewrite table installed"), "{readout}");
}

#[test]
fn the_kernel_namespace_cannot_be_aliased_away() {
    // The kernel resolves `urn:kernel:*` ahead of the root space precisely so
    // nothing can shadow it; a rewrite must not be a second door to that.
    let kernel = Kernel::new(backing_space(Arc::new(Mutex::new(String::new())))).with_aliases(
        Arc::new(AliasTable::new().prefix("urn:kernel:", "urn:iki:fn:")),
    );
    let readout =
        String::from_utf8(source(&kernel, "urn:kernel:aliases", &Capability::root()).unwrap())
            .unwrap();
    assert!(readout.starts_with("aliases"), "{readout}");
}
