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

use ikigai_conformance::{Expected, Suite};
use ikigai_core::Kernel;
use ikigai_vocab::TurtleRenderer;
use std::sync::Arc;

/// Every endpoint `space()` binds, by description id.
const ENDPOINTS: [&str; 1] = ["ikigai-vocab"];

#[test]
fn conforms() {
    let space = Arc::new(ikigai_vocab::space());
    let kernel = Kernel::with_meta_renderer(space.clone(), Arc::new(TurtleRenderer));

    let report = ENDPOINTS
        .iter()
        .fold(Suite::new(), |suite, id| suite.pure(*id).cacheable(*id))
        .body_space(space)
        .self_named_space("vocab", ikigai_vocab::space)
        .run_blocking(&kernel);
    // Printed so `--nocapture` shows what was probed and what was not, even when clean.
    println!("{report}");
    // This deterministic, input-free endpoint must exercise every applicable check.
    report.assert_clean_with(&Expected::new().no_opt_outs().nothing_unprobed());
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
