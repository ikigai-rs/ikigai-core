//! The module recipe as one test: `ikigai-conformance` walks every endpoint
//! `ikigai_vocab::space()` binds and reports every violation at once.
//!
//! - `pure` and `cacheable`: the one endpoint serves [`ikigai_vocab::VOCABULARY`], a
//!   constant compiled into the crate, so it reads nothing (no file, network, clock or
//!   platform) and a cacheable answer with no golden thread but its own name is correct.
//! - `space()` is **self-named** `urn:iki:space:vocab`: it takes no parameters and reads
//!   nothing while it is built, so every call holds the same door and the name is a
//!   true claim (ledger #987).
//!
//! The suite links this workspace's own `ikigai-core` and `ikigai-vocab` through the
//! workspace root's `[patch.crates-io]`, so the kernel built here and the one the suite
//! checks against are the same crate.

use ikigai_conformance::Suite;
use ikigai_core::Kernel;
use ikigai_vocab::TurtleRenderer;
use std::sync::Arc;

/// Every endpoint `space()` binds, by description id.
const ENDPOINTS: [&str; 1] = ["ikigai-vocab"];

#[test]
fn conforms() {
    let kernel =
        Kernel::with_meta_renderer(Arc::new(ikigai_vocab::space()), Arc::new(TurtleRenderer));

    let report = ENDPOINTS
        .iter()
        .fold(Suite::new(), |suite, id| suite.pure(*id).cacheable(*id))
        .self_named_space("vocab", ikigai_vocab::space)
        .run_blocking(&kernel);
    assert!(report.is_clean(), "{report}");
    assert_eq!(
        ikigai_core::space_iri("vocab").as_str(),
        ikigai_vocab::SPACE_ID
    );

    // The walk saw exactly the endpoints declared above: a second binding without a
    // declaration would be held to a weaker standard, and a declared id that binds
    // nothing is a stale list.
    assert_eq!(
        report.endpoints,
        ENDPOINTS.len(),
        "every binding is declared: {report}"
    );
}
