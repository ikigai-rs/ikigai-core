//! `ikigai-core` — the resolution kernel spine.
//!
//! M0 establishes the identity model that everything else resolves through:
//!
//! - [`Iri`] — validated absolute resource identifiers
//! - [`Verb`] — the request verbs and their cacheability
//! - [`ArgRef`] — by-reference / inline / content-addressed arguments
//! - [`Request`] / [`RequestId`] — a request and its content-addressed identity
//! - [`Representation`] / [`ReprType`] / [`ContentId`] — a typed value and its content address
//! - [`Capability`] — the unforgeable authority handle (shape only in M0)
//!
//! The identity model is content-addressed: equal inputs collapse to equal
//! identities, so caching and de-duplication fall out by construction.
//!
//! M1 adds resolution: a [`Request`] is matched by a [`Grammar`] within a
//! [`Space`] to an [`Endpoint`] that produces a [`Representation`]. Spaces
//! compose via [`Mount`] (union), [`Fallback`] (first hit), [`Rewrite`]
//! (preimage) and [`Limit`] (difference: a family carved out of a chain, by
//! structure).
//!
//! [`Scope`] is the resolution **chain** a request is resolved in — corridors a
//! host injects ahead of the root ([`Kernel::issue_in`]), or a chain with the
//! root cut off ([`Confine`], [`Invocation::confine`]) inside which anything
//! unbound is unresolvable rather than denied.
//!
//! A space may claim an identity (`.named(iri)`, [`Space::id`]) and reports its
//! structure ([`Space::topology`], a [`Topology`] tree), so the arrangement itself
//! is a resource: `urn:kernel:topology` renders the chain a request sees as a
//! graph, and a hit reports which space answered ([`Resolved::answered_by`]).
//!
//! [`alias`] adds **logical rewrite** on that same composition primitive: a stable
//! logical URI ([`Alias`] over an [`AliasTable`]) resolves to a different backing
//! resource without the caller knowing — the mechanism that lets a namespace
//! migration be a transition window instead of a flag day.
//!
//! [`cache`] holds what the kernel keeps: the representation cache with its
//! golden-thread validity, its bound, and the [`CachePolicy`] a host installs to
//! decide what is worth keeping.
//!
//! [`listen`] tells a host when a golden thread is cut and what the cut
//! invalidated — a bounded queue the host drains or awaits ([`Kernel::listen`]),
//! so it can recompute before the first reader or push a change to a page.
//!
//! Beside the resolution spine, [`config`] holds the config-home path algebra —
//! pure path computation, no I/O — so hosts, modules and tools that do not depend
//! on one another still agree on where configuration lives.
//!
//! # Naming conventions
//!
//! Three namespaces, three conventions — and the first two are easy to conflate,
//! which is how this section came to contradict [`Description::id`] for a while:
//!
//! - **Resource names** — a bound IRI's own segment and the matching
//!   [`Description::id`] (`tag-suggest`, `kernel-catalog`) — are a short **noun in
//!   `kebab-case`**. A noun because a resource is a thing you resolve, not a
//!   procedure you call; `kebab-case` because these are published names — the MCP
//!   projection derives an agent's tool names from the id — and they read across
//!   IRIs, shells and tool lists without casing surprises. [`Description::id`]
//!   records what is *enforced* about one (nothing but IRI-safety) as against what
//!   is convention.
//! - **RDF vocabulary terms** — the classes and properties an emitted graph uses,
//!   e.g. `ik:Endpoint`, `ik:requires` — keep RDF-idiomatic casing: `PascalCase`
//!   for classes, `lowerCamelCase` for properties. That convention belongs to the
//!   vocabulary, not to resource naming, and applying it to endpoint names is the
//!   mistake this crate made.
//! - **Rust identifiers** use `snake_case` and `PascalCase` per Rust convention.
//!
//! A constructor's `snake_case` name and its resource name therefore agree up to
//! the separator: the kernel's own operations describe themselves as `kernel-cut`,
//! `kernel-catalog`, `kernel-actions`. The exceptions are visible and known:
//! [`builtins`] still
//! carries the `lowerCamelCase` ids `toUpper`, `reverseList` and `echo` from before
//! the convention settled, and those are live MCP tool names — renaming them moves
//! an agent's tool list, so they ride one coordinated ecosystem-wide wave rather
//! than leaking out of an unrelated change.
#![forbid(unsafe_code)]

pub mod alias;
mod arg;
pub mod builtins;
pub mod cache;
mod capability;
pub mod config;
mod confine;
mod content;
mod describe;
mod endpoint;
mod error;
mod grammar;
pub(crate) mod hashing;
mod iri;
mod kernel;
mod kernel_ops;
pub mod listen;
mod meta;
mod repr;
mod request;
mod seal;
mod select;
mod space;
mod topology;
mod uncached;
mod verb;

pub use alias::{
    Alias, AliasHop, AliasParseError, AliasRefusal, AliasRule, AliasTable, Canonical, RuleKind,
    DEFAULT_MAX_HOPS,
};
pub use arg::ArgRef;
pub use cache::{
    CacheBound, CacheEntry, CacheKey, CachePolicy, CacheRow, CostAware, CutSnapshot, EntryFacts,
    EntryState, Fifo, Lru, ReprCache,
};
pub use capability::Capability;
pub use confine::Confine;
pub use content::{ContentId, ContentIdError};
pub use describe::{ActionSpec, ArgSpec, Description, EndpointKind, InputSource, Transreption};
#[cfg(not(target_family = "wasm"))]
pub use endpoint::SyncIssuer;
pub use endpoint::{
    AsyncFnEndpoint, BoxFuture, Dependencies, Endpoint, FnEndpoint, Invocation, InvokeFuture,
    Issuer, Spawner,
};
pub use error::{Error, Result};
pub use grammar::{Bindings, Exact, Grammar, TemplateError, UriTemplate};
pub use iri::{escape_iri_fragment, is_iri_safe, Iri, IriError};
pub use kernel::{
    Clock, FixedClock, Kernel, SchedulerReporter, SystemClock, TraceEvent, TraceScope, Tracer,
    ALIAS_MISS_NOTE, ALIAS_NOTE, ANSWERED_NOTE, BINDINGS_THREAD, DEFAULT_MAX_DEPTH, DENIED_NOTE,
    DEPTH_NOTE, LEVEL_NOTE, LIMITED_NOTE, META_LOSSY_ARG, SCOPE_CLOCK_NOTE, SCOPE_MISS_NOTE,
    SCOPE_NOTE, SEALED_NOTE, UNCACHED_NOTE,
};
pub use listen::{
    CutBatch, CutEvent, CutListener, ListenSpec, CAP_LISTEN, INVALIDATED_NAMED, LISTEN_CAPACITY,
};
pub use meta::MetaRenderer;
pub use repr::{Expiry, Provenance, ReprType, Representation, Thread, Time};
pub use request::{Request, RequestId};
pub use seal::{SealError, SealOwner};
pub use select::{
    is_auto_invocable, is_lossless_plan, select_action, select_action_in, select_transreptor,
    select_transreptor_in, select_transreptor_in_with, select_transreptor_with, ActionMatch,
    ActionQuery, TransreptionPolicy, TransreptionStep, CANONICAL,
};
pub use space::{
    EndpointSpace, Fallback, Level, LevelPath, Limit, Mount, Resolution, Resolved, Rewrite, Scope,
    Space, SpaceEntry,
};
pub use topology::{SpaceKind, Topology, TopologyRule};
pub use verb::Verb;
