//! **The test-only door onto a chain** (ledger #3866), behind the `testkit` feature.
//! Not API: nothing here is covered by semver, and a host or a module never enables it.
//!
//! An endpoint uses its scope only by issuing requests into it (`Invocation::scope` is
//! crate-private, and so are a chain's live corridors and its level stack). The kernel's
//! own tests still need to SEE the chain an endpoint ran in, to pin what resolution did:
//! a level stack, a confinement, a fingerprint. These functions are that probe, kept out
//! of the endpoint API so a probe is never mistaken for a capability.
//!
//! This crate's own tests enable the feature through a dev-dependency on itself, so
//! `cargo test` sees it and `cargo build` never does.

use std::sync::Arc;

use crate::endpoint::Invocation;
use crate::space::{LevelPath, Scope, Space};

/// The resolved scope `inv` runs in: the chain its sub-requests resolve against.
pub fn scope<'a>(inv: &'a Invocation<'_>) -> &'a Scope {
    inv.scope()
}

/// The corridors `scope` injected, outermost first, as live spaces.
pub fn spaces(scope: &Scope) -> &[Arc<dyn Space>] {
    scope.spaces()
}

/// The level stack `scope` consults after the host's corridors, innermost first.
pub fn levels(scope: &Scope) -> &LevelPath {
    scope.levels()
}
