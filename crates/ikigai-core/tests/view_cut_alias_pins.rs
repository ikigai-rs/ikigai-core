//! **Five kernel behaviors no test held** (ledger #3867): views, cuts, the kernel's alias
//! table, and a cycle across tables. One section each.
//!
//! 1. A `Sink` or `Delete` through a plain view writes the target, cuts the target's
//!    thread and the view's own name, and a cached read through the view recomputes.
//! 2. `Meta` of a view inside a `Level` describes the target the view's invocation
//!    reaches: the target resolved in the view's RESOLVED scope (its level first), the
//!    chain a `Source` through the view resolves it in. Before ledger #3867 `Meta`
//!    resolved the target in the REQUEST's chain and described the root's binding while
//!    `Source` reached the level's.
//! 3. A cut of an alias's LOGICAL name cuts the canonical thread too, by
//!    [`Kernel::cut`] and by `urn:kernel:cut`, as a request's target is canonicalized.
//!    Before ledger #3867 it cut only the string given, and invalidated no read.
//! 4. The kernel-held alias table rewrites a name before resolution, whatever the chain:
//!    inside a severed or a confined chain the rewritten name is what resolves there.
//! 5. A cycle through a view in one table and an alias in the kernel's table is stopped
//!    only by the nesting budget, `DepthExceeded`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, ArgRef, ArgSpec, AsyncFnEndpoint, Capability, Confine, Description,
    EndpointSpace, Error, Exact, Fallback, FnEndpoint, Iri, Kernel, Level, ListenSpec,
    MetaRenderer, Mount, ReprType, Representation, Request, Result, Scope, Space, Verb,
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

fn read(kernel: &Kernel, target: &str) -> Result<String> {
    block_on(kernel.issue(source(target), &Capability::root()))
        .map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
}

/// A constant, cacheable answer.
fn constant(name: &'static str, bytes: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(bytes).cacheable()))
}

/// A view: an alias rule that forwards `from` to `to`, resolved from the caller's chain
/// (`ik:resolveAt "root"`), over an empty child.
fn view(from: &str, to: &str) -> Arc<dyn Space> {
    Arc::new(Alias::new(
        Arc::new(AliasTable::new().exact_at_root(from, to)),
        Arc::new(EndpointSpace::new()),
    ))
}

// 1 ─────────────────────────────────────────────────────────────────────────────

const CELL: &str = "urn:example:cell";
const CELL_VIEW: &str = "urn:example:view:cell";

/// A cell: `Source` answers its value (cacheable, counting its runs), `Sink` sets it from
/// `content`, `Delete` empties it.
fn cell(state: Arc<Mutex<Vec<u8>>>, runs: Arc<AtomicU32>) -> FnEndpoint {
    FnEndpoint::new("cell", move |inv| match inv.request.verb {
        Verb::Sink => {
            *state.lock().unwrap() = inv.inline_arg("content")?.to_vec();
            Ok(text(b"written"))
        }
        Verb::Delete => {
            state.lock().unwrap().clear();
            Ok(text(b"deleted"))
        }
        _ => {
            runs.fetch_add(1, Ordering::SeqCst);
            Ok(text(&state.lock().unwrap()).cacheable())
        }
    })
}

struct Cell {
    kernel: Kernel,
    state: Arc<Mutex<Vec<u8>>>,
    runs: Arc<AtomicU32>,
}

fn cell_behind_a_view() -> Cell {
    let state = Arc::new(Mutex::new(b"old".to_vec()));
    let runs = Arc::new(AtomicU32::new(0));
    let host: Arc<dyn Space> = Arc::new(EndpointSpace::new().bind(
        Exact::new(CELL),
        cell(Arc::clone(&state), Arc::clone(&runs)),
    ));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![view(CELL_VIEW, CELL), host]));
    Cell {
        kernel: Kernel::new(root),
        state,
        runs,
    }
}

/// The threads a mutating request through the view cut, in cut order.
fn cuts_of(kernel: &Kernel, request: Request) -> Vec<String> {
    let listener = kernel
        .listen(
            ListenSpec::new().exact(CELL).exact(CELL_VIEW),
            &Capability::root(),
        )
        .unwrap();
    block_on(kernel.issue(request, &Capability::root())).unwrap();
    listener
        .drain()
        .events
        .into_iter()
        .map(|event| event.thread.as_str().to_string())
        .collect()
}

#[test]
fn a_sink_through_a_view_writes_the_target_and_cuts_both_names() {
    let c = cell_behind_a_view();
    let root = Capability::root();
    assert_eq!(read(&c.kernel, CELL_VIEW).unwrap(), "old");
    assert_eq!(read(&c.kernel, CELL_VIEW).unwrap(), "old");
    assert_eq!(
        c.runs.load(Ordering::SeqCst),
        1,
        "the second read is served"
    );
    assert!(c.kernel.is_cached(&source(CELL_VIEW), &root));

    let write =
        Request::new(Verb::Sink, iri(CELL_VIEW)).with_arg("content", ArgRef::Inline(b"new".into()));
    let cut = cuts_of(&c.kernel, write);
    assert_eq!(
        *c.state.lock().unwrap(),
        b"new",
        "the view forwarded the Sink"
    );
    // The forwarded sub-request cut the target as it returned; the kernel then cut the
    // name the caller wrote to.
    assert_eq!(cut, [CELL, CELL_VIEW]);
    assert!(!c.kernel.is_cached(&source(CELL_VIEW), &root));
    assert!(!c.kernel.is_cached(&source(CELL), &root));

    assert_eq!(
        read(&c.kernel, CELL_VIEW).unwrap(),
        "new",
        "the view recomputes"
    );
    assert_eq!(c.runs.load(Ordering::SeqCst), 2);
}

#[test]
fn a_delete_through_a_view_deletes_the_target_and_cuts_both_names() {
    let c = cell_behind_a_view();
    let root = Capability::root();
    assert_eq!(read(&c.kernel, CELL_VIEW).unwrap(), "old");
    assert_eq!(read(&c.kernel, CELL).unwrap(), "old");
    assert_eq!(
        c.runs.load(Ordering::SeqCst),
        1,
        "one entry serves both names"
    );

    let cut = cuts_of(&c.kernel, Request::new(Verb::Delete, iri(CELL_VIEW)));
    assert!(
        c.state.lock().unwrap().is_empty(),
        "the view forwarded the Delete"
    );
    assert_eq!(cut, [CELL, CELL_VIEW]);
    assert!(!c.kernel.is_cached(&source(CELL_VIEW), &root));
    assert!(!c.kernel.is_cached(&source(CELL), &root));

    assert_eq!(
        read(&c.kernel, CELL_VIEW).unwrap(),
        "",
        "the view recomputes"
    );
    assert_eq!(c.runs.load(Ordering::SeqCst), 2);
}

// 2 ─────────────────────────────────────────────────────────────────────────────

/// JSON for every face, so a `Meta` answer reads back as a `Description`.
struct Json;

impl MetaRenderer for Json {
    fn render(&self, description: &Description, _target: &ReprType) -> Result<Representation> {
        Ok(Representation::new(
            ReprType::new("application/json"),
            serde_json::to_vec(description).unwrap(),
        ))
    }
}

/// A target described with one input, `input`, so a projection says which one it is.
fn described(name: &'static str, answer: &'static [u8], input: &str) -> FnEndpoint {
    constant(name, answer).with_description(
        Description::new(name)
            .verb(Verb::Source)
            .input(ArgSpec::new(input).summary("names which binding this is")),
    )
}

#[test]
fn meta_of_a_view_inside_a_level_describes_the_target_its_source_reaches() {
    // A module `Level` holds a view onto `urn:shared:name` and its own binding of that
    // name; the root binds the same name too. The view's invocation runs in its resolved
    // scope, the level first (audit C6), so `Source` reaches the LEVEL's binding.
    let module: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        view("urn:mod:view", "urn:shared:name"),
        Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:shared:name"),
            described("level-target", b"level's", "level_input"),
        )),
    ]));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(Mount::new(
            "urn:mod:",
            Arc::new(Level::new(iri("urn:example:level:mod"), module)),
        )),
        Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:shared:name"),
            described("root-target", b"root's", "root_input"),
        )),
    ]));
    let kernel = Kernel::with_meta_renderer(root, Arc::new(Json));

    assert_eq!(read(&kernel, "urn:mod:view").unwrap(), "level's");
    assert_eq!(read(&kernel, "urn:shared:name").unwrap(), "root's");

    // `Meta` describes that same binding.
    let answer = block_on(kernel.issue(
        Request::new(Verb::Meta, iri("urn:mod:view")),
        &Capability::root(),
    ))
    .unwrap();
    let description: Description = serde_json::from_slice(&answer.bytes).unwrap();
    let inputs: Vec<&str> = description
        .actions
        .iter()
        .filter(|a| a.verb == Verb::Source)
        .flat_map(|a| a.inputs.iter().map(|i| i.name.as_str()))
        .collect();
    assert_eq!(
        inputs,
        ["level_input"],
        "Meta of a view describes the target its invocation reaches, not the root's"
    );
}

// 3 ─────────────────────────────────────────────────────────────────────────────

const LOGICAL: &str = "urn:example:old:upper";
const CANONICAL: &str = "urn:example:new:upper";

/// A kernel whose alias table renames `LOGICAL` to `CANONICAL`, bound to a counting,
/// cacheable endpoint.
fn aliased(runs: &Arc<AtomicU32>) -> Kernel {
    let runs = Arc::clone(runs);
    let root: Arc<dyn Space> = Arc::new(EndpointSpace::new().bind(
        Exact::new(CANONICAL),
        FnEndpoint::new("upper", move |_| {
            runs.fetch_add(1, Ordering::SeqCst);
            Ok(text(b"UPPER").cacheable())
        }),
    ));
    Kernel::new(root).with_aliases(Arc::new(AliasTable::new().exact(LOGICAL, CANONICAL)))
}

#[test]
fn a_cut_of_an_aliass_logical_name_cuts_its_canonical_thread() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = aliased(&runs);
    let root = Capability::root();
    read(&kernel, LOGICAL).unwrap();
    assert!(kernel.is_cached(&source(LOGICAL), &root));
    assert!(
        kernel.is_cached(&source(CANONICAL), &root),
        "one entry for both"
    );

    kernel.cut(LOGICAL);
    assert!(
        !kernel.is_cached(&source(LOGICAL), &root),
        "a cut by the logical name invalidates the read, as a write to it would"
    );
    read(&kernel, LOGICAL).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn urn_kernel_cut_of_an_aliass_logical_name_cuts_its_canonical_thread() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = aliased(&runs);
    let root = Capability::root();
    read(&kernel, LOGICAL).unwrap();
    assert!(kernel.is_cached(&source(CANONICAL), &root));

    let cut = Request::new(Verb::Sink, iri("urn:kernel:cut"))
        .with_arg("thread", ArgRef::Inline(LOGICAL.as_bytes().to_vec()));
    let answer = block_on(kernel.issue(cut, &root)).unwrap();
    assert_eq!(
        answer.bytes,
        format!("cut {LOGICAL}\n").as_bytes(),
        "the answer names the thread as given"
    );
    assert!(!kernel.is_cached(&source(CANONICAL), &root));
}

#[test]
fn a_cut_of_a_logical_name_still_cuts_a_thread_registered_under_that_name() {
    // A thread name is any string an endpoint hangs a dependency from, so the name as
    // written is cut too: canonicalizing adds a cut, it never moves one.
    let root: Arc<dyn Space> = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:example:dependent"),
        FnEndpoint::new("dependent", |inv| {
            inv.depends_on(LOGICAL);
            Ok(text(b"dependent").cacheable())
        }),
    ));
    let kernel =
        Kernel::new(root).with_aliases(Arc::new(AliasTable::new().exact(LOGICAL, CANONICAL)));
    let cap = Capability::root();
    read(&kernel, "urn:example:dependent").unwrap();
    assert!(kernel.is_cached(&source("urn:example:dependent"), &cap));

    let listener = kernel
        .listen(ListenSpec::new().prefix("urn:"), &cap)
        .unwrap();
    kernel.cut(LOGICAL);
    let cut: Vec<String> = listener
        .drain()
        .events
        .into_iter()
        .map(|e| e.thread.as_str().to_string())
        .collect();
    assert_eq!(
        cut,
        [LOGICAL, CANONICAL],
        "the name as given, then its canonical"
    );
    assert!(!kernel.is_cached(&source("urn:example:dependent"), &cap));
}

#[test]
fn a_cut_of_a_name_no_rule_matches_cuts_that_name_once() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = aliased(&runs);
    let cap = Capability::root();
    let listener = kernel
        .listen(ListenSpec::new().prefix("urn:"), &cap)
        .unwrap();
    kernel.cut(CANONICAL);
    kernel.cut("not an iri");
    let cut: Vec<String> = listener
        .drain()
        .events
        .into_iter()
        .map(|e| e.thread.as_str().to_string())
        .collect();
    assert_eq!(cut, [CANONICAL], "a canonical name is cut once");
}

// 4 ─────────────────────────────────────────────────────────────────────────────

/// A corridor binding both names, each answering which one it is.
fn corridor() -> Arc<dyn Space> {
    Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new(LOGICAL),
                constant("logical", b"corridor's logical"),
            )
            .bind(
                Exact::new(CANONICAL),
                constant("canonical", b"corridor's canonical"),
            ),
    )
}

#[test]
fn the_kernels_alias_table_rewrites_a_name_inside_a_severed_chain() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = aliased(&runs);
    let cap = Capability::root();
    let severed = || {
        Scope::empty()
            .with_named(iri("urn:ctx:corridor"), corridor())
            .sever()
    };
    // The host's table applies before resolution and ignores the chain: the logical
    // name becomes the canonical one, which the corridor answers. The corridor's own
    // binding of the logical name is never reached.
    let answer = block_on(kernel.issue_in(source(LOGICAL), &cap, severed())).unwrap();
    assert_eq!(answer.bytes, b"corridor's canonical");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the root is cut off");

    // A severed chain that binds only the logical name does not answer it.
    let only_logical = Scope::empty()
        .with_named(
            iri("urn:ctx:logical"),
            Arc::new(EndpointSpace::new().bind(Exact::new(LOGICAL), constant("l", b"logical"))),
        )
        .sever();
    let err = block_on(kernel.issue_in(source(LOGICAL), &cap, only_logical)).unwrap_err();
    assert!(
        matches!(&err, Error::Unresolved(target) if target.as_str() == CANONICAL),
        "refused at the canonical name: {err}"
    );
}

#[test]
fn the_kernels_alias_table_rewrites_a_name_inside_a_confined_chain() {
    let runs = Arc::new(AtomicU32::new(0));
    let inner = AsyncFnEndpoint::new("reader", |inv| {
        Box::pin(async move {
            let answer = inv.source(&iri(LOGICAL)).await?;
            Ok(text(&answer.bytes).cacheable())
        })
    });
    let root: Arc<dyn Space> = Arc::new(
        EndpointSpace::new()
            .bind_arc(
                Exact::new("urn:example:confined"),
                Arc::new(Confine::new(
                    iri("urn:ctx:corridor"),
                    corridor(),
                    Arc::new(inner),
                )),
            )
            .bind(Exact::new(CANONICAL), {
                let runs = Arc::clone(&runs);
                FnEndpoint::new("root", move |_| {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(text(b"root's").cacheable())
                })
            }),
    );
    let kernel =
        Kernel::new(root).with_aliases(Arc::new(AliasTable::new().exact(LOGICAL, CANONICAL)));
    // The sub-request is rewritten by the host's table, then resolved in the
    // confinement: the corridor's canonical binding, never the root's.
    assert_eq!(
        read(&kernel, "urn:example:confined").unwrap(),
        "corridor's canonical"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

// 5 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_cycle_through_a_view_and_the_kernels_alias_table_stops_at_the_depth_bound() {
    // The kernel's table renames `urn:example:a` to `urn:example:b`; a view in the root
    // forwards `urn:example:b` back to `urn:example:a`. Neither table shows the loop, and
    // the kernel's table is checked only for cycles within itself, so nothing refuses it
    // at build. ★ THE DEPTH BUDGET IS LOAD-BEARING HERE: it is the only thing that stops
    // this loop. Removing or unbounding `DEFAULT_MAX_DEPTH` turns this request into
    // unbounded recursion and a stack overflow, which aborts the process.
    let kernel = Kernel::new(view("urn:example:b", "urn:example:a")).with_aliases(Arc::new(
        AliasTable::new().exact("urn:example:a", "urn:example:b"),
    ));
    let err = read(&kernel, "urn:example:a").unwrap_err();
    assert!(matches!(err, Error::DepthExceeded { .. }), "{err}");

    // A smaller budget stops it sooner, by the same refusal.
    let kernel = Kernel::new(view("urn:example:b", "urn:example:a"))
        .with_aliases(Arc::new(
            AliasTable::new().exact("urn:example:a", "urn:example:b"),
        ))
        .with_max_depth(4);
    let err = read(&kernel, "urn:example:a").unwrap_err();
    assert!(
        matches!(err, Error::DepthExceeded { depth: 5, .. }),
        "{err:?}"
    );
}
