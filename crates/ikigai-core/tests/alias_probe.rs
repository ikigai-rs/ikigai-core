//! **The cache-status probe sees through an alias** (ledger #2503).
//!
//! The demo REPL labeled every read of `urn:httpGet` `[computed]`, and `cache
//! urn:httpGet …` said `not cached`, while the canonical `urn:http:get` said `[cached]`.
//! Nothing was refetched — an `Alias` overlay reports its rewrite
//! ([`Resolved::canonical`](ikigai_core::Resolved::canonical)) and the serving path keys
//! one entry for both names — but [`Kernel::is_cached`] keyed the probe under the
//! logical name, where nothing is ever stored. The alias module's decision 3 says the
//! two names SHARE a cache entry; the probe must say so too.
//!
//! A probe is an observer: it must never count as a resolution at
//! `urn:kernel:aliases` (no hop, no unresolved), and a refused or unresolved alias is
//! never cached.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, Capability, EndpointSpace, Exact, FixedClock, FnEndpoint, Invocation, Iri,
    Kernel, ReprType, Representation, Request, Space, Verb,
};

const LOGICAL: &str = "urn:httpGet";
const CANONICAL: &str = "urn:http:get";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target)).with_arg(
        "url",
        ikigai_core::ArgRef::Inline(b"http://www.httpbin.org/cache/120".to_vec()),
    )
}

/// An endpoint answering `cacheable_until(now + 120 s)`, counting its runs.
fn http_get(calls: &'static AtomicU32) -> EndpointSpace {
    EndpointSpace::new().bind(
        Exact::new(CANONICAL),
        FnEndpoint::new("get", move |inv: &Invocation<'_>| {
            calls.fetch_add(1, Ordering::SeqCst);
            let now = inv.now().expect("the kernel has a clock");
            Ok(
                Representation::new(ReprType::new("text/plain"), b"body".to_vec())
                    .cacheable_until(now.plus_millis(120_000)),
            )
        }),
    )
}

fn table() -> Arc<AliasTable> {
    Arc::new(AliasTable::new().exact(LOGICAL, CANONICAL))
}

/// The hub repro: an `Alias` overlay over the endpoint, on a kernel with a clock.
fn overlay(table: &Arc<AliasTable>, calls: &'static AtomicU32) -> Kernel {
    let root: Arc<dyn Space> = Arc::new(Alias::new(Arc::clone(table), Arc::new(http_get(calls))));
    Kernel::new(root).with_clock(Arc::new(FixedClock::at(1_000)))
}

/// The same table installed in the kernel itself.
fn installed(table: &Arc<AliasTable>, calls: &'static AtomicU32) -> Kernel {
    Kernel::new(Arc::new(http_get(calls)))
        .with_aliases(Arc::clone(table))
        .with_clock(Arc::new(FixedClock::at(1_000)))
}

fn counters(table: &AliasTable) -> (u64, u64, u64) {
    let rule = &table.rules()[0];
    (rule.hops(), rule.unresolved(), rule.refused())
}

/// Read under `first`, then probe both names: cached under BOTH, the endpoint ran
/// once, and the probes moved no counter.
fn read_then_probe(kernel: &Kernel, table: &AliasTable, calls: &AtomicU32, first: &str) {
    let cap = Capability::root();
    for name in [LOGICAL, CANONICAL] {
        assert!(
            !kernel.is_cached(&source(name), &cap),
            "{name} before any read"
        );
    }
    block_on(kernel.issue(source(first), &cap)).unwrap();
    let before = counters(table);
    for name in [LOGICAL, CANONICAL] {
        assert!(
            kernel.is_cached(&source(name), &cap),
            "after a read under {first}, {name} probes as cached"
        );
    }
    assert_eq!(
        counters(table),
        before,
        "a probe records no hop and no unresolved"
    );
    // And the probe told the truth: neither name runs the endpoint again.
    block_on(kernel.issue(source(LOGICAL), &cap)).unwrap();
    block_on(kernel.issue(source(CANONICAL), &cap)).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "both names served from one entry"
    );
}

#[test]
fn overlay_read_under_the_alias_probes_cached_under_both_names() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let table = table();
    read_then_probe(&overlay(&table, &CALLS), &table, &CALLS, LOGICAL);
}

#[test]
fn overlay_read_under_the_canonical_probes_cached_under_both_names() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let table = table();
    read_then_probe(&overlay(&table, &CALLS), &table, &CALLS, CANONICAL);
}

#[test]
fn installed_read_under_the_alias_probes_cached_under_both_names() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let table = table();
    read_then_probe(&installed(&table, &CALLS), &table, &CALLS, LOGICAL);
}

#[test]
fn installed_read_under_the_canonical_probes_cached_under_both_names() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let table = table();
    read_then_probe(&installed(&table, &CALLS), &table, &CALLS, CANONICAL);
}

#[test]
fn a_refused_alias_probes_uncached_and_counts_nothing() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let cyclic = Arc::new(
        AliasTable::new()
            .exact("urn:a", "urn:b")
            .exact("urn:b", "urn:a"),
    );
    let cap = Capability::root();
    let root: Arc<dyn Space> =
        Arc::new(Alias::new(Arc::clone(&cyclic), Arc::new(http_get(&CALLS))));
    let over = Kernel::new(root).with_clock(Arc::new(FixedClock::at(1_000)));
    let inst = Kernel::new(Arc::new(http_get(&CALLS)))
        .with_aliases(Arc::clone(&cyclic))
        .with_clock(Arc::new(FixedClock::at(1_000)));
    for kernel in [&over, &inst] {
        block_on(kernel.issue(source(CANONICAL), &cap)).unwrap();
        assert!(!kernel.is_cached(&source("urn:a"), &cap));
    }
    for rule in cyclic.rules() {
        assert_eq!((rule.hops(), rule.unresolved(), rule.refused()), (0, 0, 0));
    }
}

#[test]
fn an_unresolved_alias_probes_uncached_and_counts_nothing() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let dangling = Arc::new(AliasTable::new().exact("urn:httpGone", "urn:http:gone"));
    let cap = Capability::root();
    let root: Arc<dyn Space> = Arc::new(Alias::new(
        Arc::clone(&dangling),
        Arc::new(http_get(&CALLS)),
    ));
    let over = Kernel::new(root).with_clock(Arc::new(FixedClock::at(1_000)));
    let inst = Kernel::new(Arc::new(http_get(&CALLS)))
        .with_aliases(Arc::clone(&dangling))
        .with_clock(Arc::new(FixedClock::at(1_000)));
    for kernel in [&over, &inst] {
        assert!(!kernel.is_cached(&source("urn:httpGone"), &cap));
    }
    assert_eq!(counters(&dangling), (0, 0, 0));
}
