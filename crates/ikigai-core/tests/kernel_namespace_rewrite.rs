//! **No space answers in `urn:kernel:`** (ledger #750: A1, E1). The kernel answers its
//! own namespace ahead of every space — but a space may rewrite a name on its own
//! and report the result as `Resolved::canonical`, which the kernel adopts AFTER that
//! dispatch. A module that rewrote its public name into `urn:kernel:` was therefore
//! adopted as a kernel operation: its answer cached under `urn:kernel:actions`' own
//! key (the agent's tool list), its Sink auto-cutting `urn:kernel:bindings` without
//! `urn:cap:kernel:cut`. Refused now, by the seal check where a level or a canonical
//! is on the path and by the kernel's canonical adoption on every path.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, Level, Mount, ReprType,
    Representation, Request, Rewrite, Scope, Space, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(s: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), s.as_bytes().to_vec())
}

/// A module that binds `kernel_name` and rewrites `public` onto it.
fn rewriting_module(public: &'static str, kernel_name: &'static str) -> Arc<dyn Space> {
    Arc::new(Rewrite::new(
        Arc::new(EndpointSpace::new().bind(
            Exact::new(kernel_name),
            FnEndpoint::new("fake", |_| Ok(text("FAKE MANIFOLD").cacheable())),
        )),
        move |t: &Iri| (t.as_str() == public).then(|| iri(kernel_name)),
    ))
}

fn in_a_level(module: Arc<dyn Space>) -> Arc<dyn Space> {
    Arc::new(Mount::new(
        "urn:mod:",
        Arc::new(Level::new(iri("urn:example:level:mod"), module)),
    ))
}

fn in_no_level(module: Arc<dyn Space>) -> Arc<dyn Space> {
    Arc::new(Mount::new("urn:mod:", module))
}

/// A1: reading the module's public name does not serve the module's fake as the
/// action manifold — not to the caller, and not to the host's next read of the
/// real `urn:kernel:actions`, which the fake would otherwise have been cached under.
#[test]
fn a_space_rewrite_into_the_kernel_namespace_cannot_poison_the_action_manifold() {
    for (place, root) in [
        (
            "in a level",
            in_a_level(rewriting_module("urn:mod:x", "urn:kernel:actions")),
        ),
        (
            "in no level",
            in_no_level(rewriting_module("urn:mod:x", "urn:kernel:actions")),
        ),
    ] {
        let kernel = Kernel::new(root);
        let cap = Capability::root();
        let manifold = || Request::new(Verb::Source, iri("urn:kernel:actions"));
        let before = block_on(kernel.issue(manifold(), &cap)).unwrap();
        assert!(!String::from_utf8_lossy(&before.bytes).contains("FAKE"));
        kernel.bindings_changed();

        let through = block_on(kernel.issue(Request::new(Verb::Source, iri("urn:mod:x")), &cap));
        let message = format!("{through:?}");
        assert!(
            through.is_err(),
            "{place}: the rewrite was answered: {message}"
        );
        assert!(message.contains("urn:kernel:"), "{place}: {message}");

        let after = block_on(kernel.issue(manifold(), &cap)).unwrap();
        assert!(
            !String::from_utf8_lossy(&after.bytes).contains("FAKE"),
            "{place}: urn:kernel:actions was served a module's answer"
        );
    }
}

/// E1: an unprivileged `Sink` through the module's public name does not cut the
/// kernel's binding-change thread — the cut only `urn:kernel:cut` (gated by
/// `urn:cap:kernel:cut`) or the host may make.
#[test]
fn a_space_rewrite_into_the_kernel_namespace_cannot_cut_a_kernel_thread() {
    for (place, root) in [
        (
            "in a level",
            in_a_level(rewriting_module("urn:mod:y", "urn:kernel:bindings")),
        ),
        (
            "in no level",
            in_no_level(rewriting_module("urn:mod:y", "urn:kernel:bindings")),
        ),
    ] {
        let kernel = Kernel::new(root);
        let root_cap = Capability::root();
        let manifold = || Request::new(Verb::Source, iri("urn:kernel:actions"));
        block_on(kernel.issue(manifold(), &root_cap)).unwrap();
        assert!(kernel.is_cached(&manifold(), &root_cap));

        let nobody = Capability::scoped(Vec::<String>::new());
        let direct = Request::new(Verb::Sink, iri("urn:kernel:cut"))
            .with_arg("content", ArgRef::Inline(b"urn:kernel:bindings".to_vec()));
        assert!(block_on(kernel.issue(direct, &nobody)).is_err());

        let sink = Request::new(Verb::Sink, iri("urn:mod:y"))
            .with_arg("content", ArgRef::Inline(b"x".to_vec()));
        assert!(
            block_on(kernel.issue(sink, &nobody)).is_err(),
            "{place}: the rewrite was answered"
        );
        assert!(
            kernel.is_cached(&manifold(), &root_cap),
            "{place}: an unprivileged Sink cut the kernel's binding-change thread"
        );
    }
}

/// The host's injected corridors skip the seal check (host authority), so the
/// kernel's own adoption check is the one that holds there.
#[test]
fn a_corridor_rewrite_into_the_kernel_namespace_is_refused_too() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let corridor = rewriting_module("urn:mod:x", "urn:kernel:actions");
    let scope = Scope::empty().with(corridor);
    let cap = Capability::root();
    let through =
        block_on(kernel.issue_in(Request::new(Verb::Source, iri("urn:mod:x")), &cap, scope));
    assert!(
        through.is_err(),
        "a corridor's rewrite was answered: {through:?}"
    );
}

/// The kernel's OWN alias table may still point a name at a kernel resource: it
/// rewrites before the `urn:kernel:*` dispatch, so the kernel answers it.
#[test]
fn the_kernels_own_alias_into_the_namespace_still_reaches_the_kernel() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new())).with_aliases(Arc::new(
        ikigai_core::AliasTable::new().exact("urn:my:actions", "urn:kernel:actions"),
    ));
    let cap = Capability::root();
    let via =
        block_on(kernel.issue(Request::new(Verb::Source, iri("urn:my:actions")), &cap)).unwrap();
    let direct =
        block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:actions")), &cap))
            .unwrap();
    assert_eq!(via.bytes, direct.bytes);
}
