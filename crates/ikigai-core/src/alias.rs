//! **Logical rewrite** — a stable logical URI that resolves to a different backing
//! resource, without the caller knowing.
//!
//! `urn:log:config` → `file:/logConfig.yaml` (an *exact* alias: one name, one
//! backing thing). `urn:fn:` → `urn:iki:fn:` (a *prefix* alias: a whole namespace
//! moves, and every name under it follows). Same mechanism; exact is the
//! degenerate prefix.
//!
//! ## Why it exists
//!
//! An audit found 98 unregistered `urn:` namespace identifiers across the
//! ecosystem, with an IANA-registered one (`urn:isbn:`) already arriving in the
//! same graph from Zotero. Consolidating under a single `urn:iki:` namespace
//! without this primitive is a flag day: nine repos, seventeen published crates,
//! two live machines and a deployed browser demo all changing identity at once.
//! With it, old names keep resolving through a transition window and each repo
//! moves at its own pace.
//!
//! ## Shape
//!
//! [`Alias`] is a `Space` decorator in the **interception-overlay family** — the
//! same shape as `ikigai-throttle`'s `RateLimit`/`Timeout`: wrap a space, add a
//! cross-cutting behavior to every resolution flowing through it, leave the
//! wrapped space unaware. It is deliberately *not* a second composition
//! mechanism.
//!
//! Hosts that resolve against a bare `Space` wrap it directly. Hosts running a
//! [`Kernel`](crate::Kernel) install the table with
//! [`Kernel::with_aliases`](crate::Kernel::with_aliases), which both wraps the
//! root **and** teaches the kernel the table — the kernel wants it in its own
//! hands for the readout at `urn:kernel:aliases`, for refusing a cyclic table
//! before dispatch, and for attributing a miss to the rule that moved the name.
//!
//! It is no longer needed for *identity*. This overlay reports every rewrite it
//! performs on [`Resolved::canonical`](crate::Resolved::canonical), and the kernel
//! adopts the reported name before it computes the cache key or fires the
//! golden-thread cut. So an `Alias` composed by hand under another overlay —
//! `RateLimit::new(Alias::new(table, space))` — shares one cache entry and one
//! thread across both names, which it did not before 0.1.64.
//!
//! ★ **`urn:fn:` → `urn:iki:fn:` is the one real migration, and it stays.** Every
//! other namespace in this crate's examples and tests is fictional and lives under
//! the IANA-reserved `urn:example:` (RFC 6963), because `grep -r urn:iki:` is how
//! the ecosystem answers "what has actually moved?" — a fixture naming a namespace
//! that does not exist makes that grep lie, and a fixture naming the migrated form
//! of `urn:kernel:` would one day be **ambiguous with production usage** rather
//! than merely fictional. Teaching the alias with
//! the thing it was actually built for is worth more than grep purity, so do not
//! "finish the job" by moving this pair too. `tests/namespaces.rs` enforces the
//! split.
//!
//! ```
//! use std::sync::Arc;
//! use ikigai_core::{AliasTable, Iri};
//!
//! let table = AliasTable::new().prefix("urn:fn:", "urn:iki:fn:");
//! let hop = table.canonicalize(&Iri::parse("urn:fn:toUpper").unwrap());
//! assert_eq!(hop.canonical().unwrap().as_str(), "urn:iki:fn:toUpper");
//! # let _ = Arc::new(table);
//! ```
//!
//! ## ★ The five decisions, and why
//!
//! A rewrite primitive goes wrong in five specific places. None of them is
//! answerable by "whatever falls out of the implementation", so each is forced
//! here and enforced by a test.
//!
//! **1. Rewrite happens BEFORE the capability check.** Authority is *also* named
//! by URN (`urn:cap:fs:read:…`), so the ordering is a security decision, not a
//! plumbing one. The kernel canonicalizes the target first and evaluates the
//! declared-capability floor against the **backing** resource — the thing that
//! will actually be touched. The other order is an escalation device: check
//! `urn:public:thing`, then rewrite to `urn:secret:thing`, and the grant that was
//! examined is not the grant that was needed. A *silent grant* is strictly worse
//! than a silent denial, so the order is fixed in the safe direction.
//!
//! The cost of that choice is the failure the ecosystem has already paid for
//! twice this month: a caller holding a grant that names the **pre-alias** scope
//! now fails a check against the post-alias target, by simply not holding it,
//! with nothing in any log. So this module refuses to let that be silent — see
//! *Observability* below. Note what is deliberately **not** done: the table is
//! never applied to capability scopes. Rewriting authority to follow a name is
//! how you manufacture the silent grant; migrating grants is an explicit act by
//! whoever mints them.
//!
//! **2. `Meta` reports the BACKING name.** Self-description that lies breaks the
//! catalog, selection and the MCP projection downstream. The canonical target is
//! adopted before dispatch, so the description an endpoint returns — and the
//! `Description::id` the catalog and `urn:kernel:actions` key on — is the
//! backing one, everywhere, with no special case. The alias is disclosed as
//! *provenance* (a trace note), never by editing the description.
//!
//! **3. The two names SHARE a cache entry and a golden thread.** They must: a
//! `Sink` through one name that left the other serving a stale representation is
//! the invalidation bug that is hardest to see, because both answers look
//! plausible. The representation cache keys on [`Request::id`](crate::Request::id)
//! and the auto-cut on `request.target`, so identity is settled at one point on
//! the resolution path — for the cache key, the auto-cut, the trace event and the
//! `Invocation` the endpoint sees alike.
//!
//! It is settled by the **resolver reporting what it rewrote**, not by the kernel
//! knowing the table in advance. This overlay stamps
//! [`Resolved::canonical`](crate::Resolved::canonical); the kernel adopts it
//! before the id is computed. The earlier design — canonicalize at the top from a
//! table the kernel holds — is still there (it is what refuses a cycle *before*
//! dispatch and what attributes a miss), but it was the *only* mechanism, and it
//! only ever fired for a table installed through
//! [`Kernel::with_aliases`](crate::Kernel::with_aliases). Composed by hand under
//! another overlay, the rewrite happened and the canonicalization did not: two
//! cache entries and two golden threads over one resource, with
//! `(no rewrite table installed)` in a readout nobody reads as the only signal.
//! Reporting closes that, because the report travels with the resolution however
//! the space was assembled.
//!
//! **4. The catalog lists the BACKING name, once.** [`Alias::entries`] is
//! transparent — it forwards the wrapped space's entries unchanged. Listing both
//! names over-offers: two IRIs for one resource, an agent picking arbitrarily,
//! and selection dedup (which keys on description id) quietly collapsing them
//! anyway. Listing the alias *instead* would hide the name the system is moving
//! to. The migration property this buys is the one that matters: during the
//! transition the catalog advertises only the **new** names, so every consumer
//! that discovers resources by reading the catalog migrates forward on its own,
//! while every consumer still holding an old name keeps working. Aliases are not
//! hidden — they are a resource of their own, at `urn:kernel:aliases`. A rule that
//! resolves from the ROOT is not a renaming: a view or a named request is a name of its
//! own, listed, and described by its target's contract (ledger #2513).
//!
//! **5. Chains resolve; cycles are refused, loudly.** `A→B→C` is followed
//! transitively up to [`AliasTable::max_hops`] (default
//! [`DEFAULT_MAX_HOPS`]) — a namespace migration layered on an earlier one is a
//! real shape, and refusing it would force operators to pre-compose their tables
//! by hand. `A→B, B→A` is detected by a visited set and **refused** with an error
//! naming the trail; it is never silently truncated to a partial rewrite, which
//! would resolve to whichever name the loop happened to stop on. Over-long and
//! malformed rewrites take the same refusal path, for the same reason: a
//! half-applied rewrite is a wrong answer that looks like a right one.
//!
//! ## Observability
//!
//! A rewrite the operator cannot see is a bad afternoon waiting to happen
//! ("it resolves under the old name but not the new one"). Three channels, in
//! increasing order of how much has to be switched on:
//!
//! - **Always on** — per-rule counters (`hops`, `unresolved`, `refused`),
//!   readable at `urn:kernel:aliases` beside the rules themselves. A rule that
//!   fired and then missed says exactly that, with no tracer installed.
//! - **When a tracer is installed** — every [`TraceEvent`](crate::TraceEvent) for
//!   an aliased request carries a note under
//!   [`ALIAS_NOTE`](crate::ALIAS_NOTE) reading `logical -> canonical`, including
//!   the denial event (which names something that never ran) and the miss.
//! - **In the error text** — a capability denial on an aliased target names the
//!   hop, and adds an explicit hint when the caller's capability holds a scope
//!   mentioning the pre-alias prefix. That is the exact silent-failure shape from
//!   decision 1, turned into a sentence.

use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::arg::ArgRef;
use crate::declare::DeclarationError;
use crate::describe::{ArgSpec, Description, InputSource};
use crate::endpoint::{Endpoint, FnEndpoint, Invocation};
use crate::error::Error;
use crate::grammar::{Bindings, Grammar, UriTemplate};
use crate::iri::Iri;
use crate::repr::Representation;
use crate::request::Request;
use crate::space::{answered, Resolution, Resolved, Scope, Space, SpaceEntry};
use crate::topology::{MatchKind, SpaceKind, Topology, TopologyRule};
use crate::verb::Verb;

/// How many rewrites one canonicalization will follow before refusing. Chains are
/// legitimate (a namespace migrated twice); an unbounded walk is not.
pub const DEFAULT_MAX_HOPS: usize = 8;

// The reserved kernel-behavior namespace (`crate::kernel::kernel_op`). Targets under
// it are **never** aliased: the kernel resolves `urn:kernel:*` itself, ahead of the
// root space, precisely so nothing can shadow it — and a rewrite that could re-point
// `urn:kernel:cut` would be exactly that shadowing, arriving through a different door.
// The test is the kernel's own, so a prefix in another case (`URN:KERNEL:cut`, the same
// name under RFC 8141) is reserved too. Aliasing *to* a kernel resource is fine;
// aliasing one *away* is not.

/// How a rule matches: the whole name, or a leading namespace.
///
/// ⚠ A TEMPLATE rule (a named request whose logical name is a URI template, ledger #2512)
/// reports [`Exact`](RuleKind::Exact) here, because it matches one whole name at a time,
/// and says it is a template through [`AliasRule::match_kind`] and
/// [`TopologyRule::template`](crate::TopologyRule::template). This enum is not
/// `#[non_exhaustive]`, so a third variant would break every exhaustive `match` on it
/// downstream; making it one is a change for the next minor version.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuleKind {
    /// The target must equal `from` exactly. `urn:log:config` → `file:/logConfig.yaml`.
    Exact,
    /// The target must start with `from`; the remainder is carried over.
    /// `urn:fn:` → `urn:iki:fn:` rewrites `urn:fn:toUpper` to `urn:iki:fn:toUpper`.
    Prefix,
}

impl RuleKind {
    /// The keyword this kind is written with in the table's text form.
    pub fn keyword(&self) -> &'static str {
        match self {
            RuleKind::Exact => "exact",
            RuleKind::Prefix => "prefix",
        }
    }
}

/// Where a rule's rewritten name is resolved: in the [`Alias`] space's own child, or
/// from the root of the caller's resolution chain (ledger #2061).
///
/// `#[non_exhaustive]`: another place to resolve at (an enclosing level, say) is
/// additive.
///
/// ```
/// use ikigai_core::{AliasTable, ResolveAt};
///
/// let table = AliasTable::new()
///     .exact("urn:example:old", "urn:example:new")
///     .exact_at_root("urn:example:view:open", "urn:example:ledger:open");
/// let at: Vec<&str> = table.rules().iter().map(|rule| rule.resolve_at().keyword()).collect();
/// assert_eq!(at, ["root", "child"]);
/// assert_eq!(ResolveAt::default(), ResolveAt::Child);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
#[non_exhaustive]
pub enum ResolveAt {
    /// In the alias's own child: the rewrite is a change of NAME inside one space, and
    /// is reported on [`Resolved::canonical`](crate::Resolved::canonical), so both
    /// names share one cache entry and one golden thread. Every rule was this one before views (ledger #2061).
    #[default]
    Child,
    /// From the ROOT of the caller's resolution chain, as a sub-request under the
    /// caller's own capability — a VIEW onto a name served anywhere the caller can
    /// reach. The match resolves to a small forwarding endpoint that re-issues the
    /// rewritten name ([`Invocation::issue`](crate::Invocation::issue)); see [`Alias`]
    /// for what that means for authority, caching and cycles.
    Root,
}

impl ResolveAt {
    /// The keyword this place is written with: `child` or `root` (the text form's
    /// optional fourth field, and the topology's `ik:resolveAt`).
    pub fn keyword(&self) -> &'static str {
        match self {
            ResolveAt::Child => "child",
            ResolveAt::Root => "root",
        }
    }
}

/// How a named request gives one argument of its target (ledger #2512): `ik:value`,
/// `ik:ref`, `ik:refTemplate` or `ik:capture` on the rule's `ik:Argument`.
///
/// `#[non_exhaustive]`: another way to give a value is additive.
///
/// ```
/// use ikigai_core::{FixedValue, Iri};
///
/// assert_eq!(FixedValue::Value("urn:example:graph".into()).to_string(), "urn:example:graph");
/// let query = Iri::parse("urn:example:queries/count.rq").unwrap();
/// assert_eq!(FixedValue::Reference(query).to_string(), "<urn:example:queries/count.rq>");
/// assert_eq!(FixedValue::Capture("id".into()).to_string(), "{id}");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FixedValue {
    /// By value: the literal, sent inline.
    Value(String),
    /// By reference: the IRI, sent as [`ArgRef::Reference`].
    Reference(Iri),
    /// By reference: an IRI template filled from captures with RFC 6570 simple
    /// string expansion. Empty captures and captures forming dot path segments
    /// are refused; reserved characters are encoded as data.
    ReferenceTemplate(UriTemplate),
    /// The WHOLE text one of the rule's template variables captured, sent by value. Never
    /// spliced inside a longer value: that would be string interpolation into a language.
    Capture(String),
}

impl fmt::Display for FixedValue {
    /// The spelling a binding writes: a value as it is, a reference as `<iri>`, a capture
    /// as `{var}`, and a reference template as `<template>` — what [`ArgSpec::fixed`](crate::ArgSpec::fixed) carries.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixedValue::Value(value) => f.write_str(value),
            FixedValue::Reference(iri) => write!(f, "<{}>", iri.as_str()),
            FixedValue::ReferenceTemplate(template) => write!(f, "<{}>", template.source()),
            FixedValue::Capture(var) => write!(f, "{{{var}}}"),
        }
    }
}

/// One argument a named request fixes (ledger #2512): its name and how it is given.
/// What the caller may not give, because it is part of what the name MEANS.
///
/// `#[non_exhaustive]`: build one with [`value`](Self::value),
/// [`reference`](Self::reference), [`reference_template`](Self::reference_template)
/// or [`capture`](Self::capture).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FixedArgument {
    /// The target's input it fills.
    pub name: String,
    /// How the value is given.
    pub value: FixedValue,
}

impl FixedArgument {
    /// Fix `name` to a literal value.
    pub fn value(name: impl Into<String>, value: impl Into<String>) -> Self {
        FixedArgument {
            name: name.into(),
            value: FixedValue::Value(value.into()),
        }
    }

    /// Fix `name` to the resource at `iri`, passed by reference.
    pub fn reference(name: impl Into<String>, iri: Iri) -> Self {
        FixedArgument {
            name: name.into(),
            value: FixedValue::Reference(iri),
        }
    }

    /// Fix `name` to a reference IRI template filled from the rule's captures.
    /// The rule validates its variables and IRI shape when bound.
    ///
    /// ```
    /// use ikigai_core::{FixedArgument, UriTemplate};
    /// let query = FixedArgument::reference_template(
    ///     "query", UriTemplate::parse("urn:example:searches/{file}").unwrap());
    /// assert_eq!(query.value.to_string(), "<urn:example:searches/{file}>");
    /// ```
    pub fn reference_template(name: impl Into<String>, template: UriTemplate) -> Self {
        FixedArgument {
            name: name.into(),
            value: FixedValue::ReferenceTemplate(template),
        }
    }

    /// Fix `name` to what the rule's template variable `var` captured.
    pub fn capture(name: impl Into<String>, var: impl Into<String>) -> Self {
        FixedArgument {
            name: name.into(),
            value: FixedValue::Capture(var.into()),
        }
    }
}

/// One rewrite rule, with the live counters that make it observable.
#[derive(Debug)]
pub struct AliasRule {
    kind: RuleKind,
    from: String,
    to: String,
    at: ResolveAt,
    /// The rule's part as a NAMED REQUEST (ledger #2512): empty for a plain rewrite.
    request: RequestPart,
    hops: AtomicU64,
    unresolved: AtomicU64,
    refused: AtomicU64,
}

/// What makes a root rule a named request: the template its logical name is, the verb it
/// forwards, the arguments it fixes and the inputs it declares. All empty for a plain
/// rewrite, which then behaves exactly as it did before named requests.
#[derive(Clone, Debug, Default)]
struct RequestPart {
    /// The logical name as a URI template, for a template rule.
    template: Option<UriTemplate>,
    /// The canonical name as a URI template, when it holds variables.
    target: Option<UriTemplate>,
    verb: Option<Verb>,
    fixes: Vec<FixedArgument>,
    inputs: Vec<ArgSpec>,
}

impl RequestPart {
    /// Whether this part carries anything a plain rewrite does not.
    fn is_empty(&self) -> bool {
        self.template.is_none()
            && self.verb.is_none()
            && self.fixes.is_empty()
            && self.inputs.is_empty()
    }
}

impl AliasRule {
    /// How this rule matches.
    pub fn kind(&self) -> RuleKind {
        self.kind
    }

    /// The logical name (or namespace) being rewritten.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// The backing name (or namespace) it is rewritten to.
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Where the rewritten name is resolved: the alias's child, or the root.
    pub fn resolve_at(&self) -> ResolveAt {
        self.at
    }

    /// How this rule matches, a template rule included ([`MatchKind::Template`]), where
    /// [`kind`](Self::kind) can only say exact or prefix.
    pub fn match_kind(&self) -> MatchKind {
        match (&self.request.template, self.kind) {
            (Some(_), _) => MatchKind::Template,
            (None, RuleKind::Exact) => MatchKind::Exact,
            (None, RuleKind::Prefix) => MatchKind::Prefix,
        }
    }

    /// The one verb a named request forwards (beside `Exists` and `Meta`), or `None` for
    /// a rule that forwards every verb (ledger #2512).
    pub fn verb(&self) -> Option<Verb> {
        self.request.verb
    }

    /// The arguments a named request fixes, in name order (ledger #2512).
    pub fn fixes(&self) -> &[FixedArgument] {
        &self.request.fixes
    }

    /// The inputs a named request declares itself, in name order (ledger #2512).
    pub fn inputs(&self) -> &[ArgSpec] {
        &self.request.inputs
    }

    /// Whether this rule is a named request: a template, a bound verb, a fixed argument or
    /// a declared input (ledger #2512). A plain rewrite is not.
    pub fn is_named_request(&self) -> bool {
        !self.request.is_empty()
    }

    /// The rule as the topology reports it.
    pub(crate) fn summary(&self) -> TopologyRule {
        let mut rule = TopologyRule::new(self.kind, self.from.as_str(), self.to.as_str())
            .resolving_at(self.at);
        rule.template = self.request.template.is_some();
        rule.verb = self.request.verb;
        rule.fixes = self.request.fixes.clone();
        rule.inputs = self.request.inputs.clone();
        rule
    }

    /// How many rewrites this rule has performed.
    pub fn hops(&self) -> u64 {
        self.hops.load(Ordering::Relaxed)
    }

    /// How many of those rewrites landed on a target nothing was bound to. The
    /// always-on answer to "it resolves under the old name but not the new one":
    /// a nonzero count here names the rule that moved it.
    pub fn unresolved(&self) -> u64 {
        self.unresolved.load(Ordering::Relaxed)
    }

    /// How many canonicalizations this rule participated in that were refused (a
    /// cycle, an over-long chain, or a substitution that is not a valid IRI).
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Apply this rule to `target`, or `None` if it does not match.
    pub(crate) fn apply(&self, target: &str) -> Option<String> {
        self.apply_capturing(target).map(|(next, _)| next)
    }

    /// Apply this rule to `target`, with what a template rule captured (empty for any
    /// other rule), or `None` if it does not match.
    ///
    /// A capture is IRI text already, so it is substituted into a templated canonical name
    /// as it is: its own `%XX` escapes are kept rather than encoded twice, and the kernel
    /// never parses arguments out of a name, so a `?` in a capture carries nothing.
    fn apply_capturing(&self, target: &str) -> Option<(String, Bindings)> {
        if let Some(template) = &self.request.template {
            let captures = template.match_iri(&Iri::parse(target).ok()?).or_else(|| {
                let empty: Vec<&str> = self
                    .request
                    .fixes
                    .iter()
                    .filter_map(|argument| match &argument.value {
                        FixedValue::ReferenceTemplate(t) => Some(t),
                        _ => None,
                    })
                    .flat_map(UriTemplate::variables)
                    .collect();
                if empty.is_empty() {
                    None
                } else {
                    template.match_with_empty(target, &empty)
                }
            })?;
            let next = match &self.request.target {
                Some(to) => to.expand(&captures)?,
                None => self.to.clone(),
            };
            return Some((next, captures));
        }
        let next = match self.kind {
            RuleKind::Exact => (target == self.from).then(|| self.to.clone())?,
            RuleKind::Prefix => target
                .strip_prefix(self.from.as_str())
                .map(|rest| format!("{}{rest}", self.to))?,
        };
        Some((next, Bindings::default()))
    }
}

/// A completed rewrite: the name the caller used, the name it resolves to, and
/// the rules it traveled through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasHop {
    logical: Iri,
    canonical: Iri,
    rules: Vec<usize>,
}

impl AliasHop {
    /// A rewrite a `Space` performed and **reported** on its
    /// [`Resolved::canonical`](crate::Resolved::canonical), rather than one this
    /// table walked. It carries no rules — nothing in this table did it, so there
    /// is nothing to attribute a counter to — and exists so the trace note, the
    /// denial message and the miss path read the same whichever half of the system
    /// did the rewriting.
    pub fn reported(logical: Iri, canonical: Iri) -> Self {
        AliasHop {
            logical,
            canonical,
            rules: Vec::new(),
        }
    }

    /// The name as the caller wrote it.
    pub fn logical(&self) -> &Iri {
        &self.logical
    }

    /// The backing name it resolves to — the kernel's identity for this request.
    pub fn canonical(&self) -> &Iri {
        &self.canonical
    }

    /// The indices (into [`AliasTable::rules`]) of the rules applied, in order.
    pub fn rules(&self) -> &[usize] {
        &self.rules
    }
}

impl fmt::Display for AliasHop {
    /// `urn:fn:toUpper -> urn:iki:fn:toUpper` — the form carried in a trace note
    /// and in a denial message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.logical, self.canonical)
    }
}

/// A rewrite that could not be completed, and is therefore **refused** rather
/// than half-applied. A partial rewrite resolves to whichever name the walk
/// happened to stop on — a wrong answer wearing a right answer's face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasRefusal {
    /// The name the caller used.
    pub logical: Iri,
    /// Why the walk stopped: a cycle, the hop limit, or a malformed substitution.
    pub reason: String,
    /// Every name visited, in order — enough to read the loop off the message.
    pub trail: Vec<String>,
    /// The rules that participated, for counter attribution.
    rules: Vec<usize>,
}

impl fmt::Display for AliasRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.reason, self.trail.join(" -> "))
    }
}

/// The outcome of canonicalizing one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Canonical {
    /// No rule matched — resolve under the name as given. The overwhelmingly
    /// common case, and the one that must cost nothing.
    Direct,
    /// Rewritten, terminating.
    Aliased(AliasHop),
    /// Deliberately refused; see [`AliasRefusal`].
    Refused(AliasRefusal),
}

impl Canonical {
    /// The name to resolve under, if the rewrite completed. `None` for
    /// [`Direct`](Canonical::Direct) (nothing changed) and for
    /// [`Refused`](Canonical::Refused) (nothing may be resolved).
    pub fn canonical(&self) -> Option<&Iri> {
        match self {
            Canonical::Aliased(hop) => Some(hop.canonical()),
            _ => None,
        }
    }

    /// The hop, if one was taken.
    pub fn hop(&self) -> Option<&AliasHop> {
        match self {
            Canonical::Aliased(hop) => Some(hop),
            _ => None,
        }
    }
}

/// An error parsing an alias table's text form.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AliasParseError {
    /// The 1-based line number.
    pub line: usize,
    /// What was wrong with it.
    pub detail: String,
}

impl fmt::Display for AliasParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "alias table line {}: {}", self.line, self.detail)
    }
}

impl std::error::Error for AliasParseError {}

/// The rewrite table: an ordered set of rules plus the hop budget.
///
/// **The table is a resource, not a hardcode.** It round-trips through a
/// line-oriented text form ([`parse`](AliasTable::parse) /
/// [`to_text`](AliasTable::to_text)) so a host can source it from a bound IRI
/// like anything else, and a kernel serves its live contents — counters and all —
/// at `urn:kernel:aliases`.
///
/// ```
/// use ikigai_core::AliasTable;
/// let table = AliasTable::parse(
///     "# the urn:iki: migration\nprefix urn:fn: urn:iki:fn:\nexact urn:log:config file:/logConfig.yaml\n",
/// )
/// .unwrap();
/// assert_eq!(table.rules().len(), 2);
/// ```
///
/// **The table's order is precedence, and it is SORTED, not authored.** A name is
/// rewritten by the FIRST rule in [`rules`](AliasTable::rules) that matches it, and
/// every insertion re-sorts the rules most specific first: the longest `from` first;
/// at equal length an exact rule before a prefix one; then by `from`. The sort is
/// stable, so two rules with the same kind and `from` keep the order they were added
/// in, and only the first can ever apply. Authored order therefore carries no
/// meaning, and "table order" — here, in [`Topology`]'s
/// `ik:rewrites`, and in a declaration's `{alias}:rule:{n}` numbering — always means
/// this sorted order.
///
/// ```
/// use ikigai_core::AliasTable;
///
/// // Authored broad first; the table puts the specific rule ahead of it anyway.
/// let table = AliasTable::new()
///     .prefix("urn:fn:", "urn:iki:fn:")
///     .exact("urn:fn:toUpper", "urn:iki:fn:upper");
/// let order: Vec<&str> = table.rules().iter().map(|rule| rule.from()).collect();
/// assert_eq!(order, ["urn:fn:toUpper", "urn:fn:"]);
/// ```
#[derive(Debug)]
pub struct AliasTable {
    rules: Vec<AliasRule>,
    max_hops: usize,
}

impl Default for AliasTable {
    fn default() -> Self {
        AliasTable::new()
    }
}

impl AliasTable {
    /// An empty table.
    pub fn new() -> Self {
        AliasTable {
            rules: Vec::new(),
            max_hops: DEFAULT_MAX_HOPS,
        }
    }

    /// Rewrite the single name `from` to `to` (builder).
    pub fn exact(self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.rule(RuleKind::Exact, from, to)
    }

    /// Rewrite every name under the namespace `from` to the same name under `to`
    /// (builder) — the case a namespace migration needs.
    pub fn prefix(self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.rule(RuleKind::Prefix, from, to)
    }

    fn rule(self, kind: RuleKind, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.rule_at(kind, from, to, ResolveAt::Child)
    }

    /// Add a rule as the topology states it (builder, ledger #2512): the form that carries
    /// a NAMED REQUEST — a template, a bound verb, fixed arguments and declared inputs —
    /// beside every plain rule [`rule_at`](Self::rule_at) builds.
    ///
    /// Refused ([`DeclarationError::Malformed`], or [`DeclarationError::Pattern`] for a
    /// template that does not parse), naming the rule's logical name, when the rule cannot
    /// mean one thing: a named request that does not resolve from the root, a template
    /// prefix rule or one with no variable, a canonical template using a variable the
    /// logical one does not capture, a `Meta` verb, a fixed or declared name given twice or
    /// not IRI-safe, a fixed `as` (it negotiates the face: declare it as an input with a
    /// default), a capture of a variable the template does not have, or a declared input
    /// that is a binding or carries more than a declaration can state.
    ///
    /// ```
    /// use ikigai_core::{AliasTable, FixedArgument, Iri, MatchKind, TopologyRule, Verb};
    ///
    /// let count = TopologyRule::named_request("urn:example:quakes:count", "urn:example:sparql")
    ///     .verb(Verb::Source)
    ///     .fixing(FixedArgument::value("graph", "urn:example:quakes:graph"))
    ///     .fixing(FixedArgument::reference(
    ///         "query",
    ///         Iri::parse("urn:example:queries/count.rq").unwrap(),
    ///     ));
    /// let event = TopologyRule::named_template("urn:example:quakes:event:{id}", "urn:example:compose")
    ///     .fixing(FixedArgument::capture("id", "id"));
    /// let table = AliasTable::new().with_rule(count).unwrap().with_rule(event).unwrap();
    /// assert_eq!(table.rules()[0].match_kind(), MatchKind::Template);
    /// assert_eq!(table.rules()[1].fixes().len(), 2);
    ///
    /// // `as` is negotiation, not input: never fixed.
    /// let face = TopologyRule::named_request("urn:example:x", "urn:example:y")
    ///     .fixing(FixedArgument::value("as", "text/csv"));
    /// assert!(AliasTable::new().with_rule(face).is_err());
    /// ```
    pub fn with_rule(self, rule: TopologyRule) -> Result<Self, DeclarationError> {
        let request = request_part(&rule)?;
        Ok(self.insert(AliasRule {
            kind: rule.kind,
            from: rule.from,
            to: rule.to,
            at: rule.at,
            request,
            hops: AtomicU64::new(0),
            unresolved: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        }))
    }

    /// Rewrite the single name `from` to `to`, and resolve `to` FROM THE ROOT under the
    /// caller's capability rather than in the alias's child (builder): a view onto a
    /// name served elsewhere. See [`ResolveAt::Root`].
    pub fn exact_at_root(self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.rule_at(RuleKind::Exact, from, to, ResolveAt::Root)
    }

    /// Rewrite every name under `from` to the same name under `to`, resolved FROM THE
    /// ROOT under the caller's capability (builder). See [`ResolveAt::Root`].
    pub fn prefix_at_root(self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.rule_at(RuleKind::Prefix, from, to, ResolveAt::Root)
    }

    /// A rule of `kind`, resolved `at` (builder): the general form of the four above.
    pub fn rule_at(
        self,
        kind: RuleKind,
        from: impl Into<String>,
        to: impl Into<String>,
        at: ResolveAt,
    ) -> Self {
        self.insert(AliasRule {
            kind,
            from: from.into(),
            to: to.into(),
            at,
            request: RequestPart::default(),
            hops: AtomicU64::new(0),
            unresolved: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        })
    }

    /// Insert one rule in precedence order.
    fn insert(mut self, rule: AliasRule) -> Self {
        self.rules.push(rule);
        // Most specific first: the longest `from` wins, and at equal length an
        // exact rule outranks a prefix one, and both outrank a template. So a table
        // carrying both `prefix urn:fn:` and `exact urn:fn:toUpper` sends `toUpper` to
        // its own destination and everything else under the namespace rule, without the
        // author having to think about declaration order.
        self.rules.sort_by(|a, b| {
            b.from
                .len()
                .cmp(&a.from.len())
                .then_with(|| a.match_kind().keyword().cmp(b.match_kind().keyword()))
                .then_with(|| a.from.cmp(&b.from))
        });
        self
    }

    /// Set the hop budget for a chain (builder). `0` is clamped to `1`: a table
    /// with rules that never applies any of them is a configuration that lies.
    pub fn with_max_hops(mut self, hops: usize) -> Self {
        self.max_hops = hops.max(1);
        self
    }

    /// The hop budget.
    pub fn max_hops(&self) -> usize {
        self.max_hops
    }

    /// The rules in precedence order, most specific first — sorted on every
    /// insertion, never as authored (see [`AliasTable`]).
    pub fn rules(&self) -> &[AliasRule] {
        &self.rules
    }

    /// Whether the table holds no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Resolve `target` to the name it should be resolved under, following chains
    /// and refusing cycles.
    ///
    /// Not a pure function: it bumps the per-rule counters that make a rewrite
    /// observable without a tracer. That is deliberate — the counters have to be
    /// on the path everything takes, or they only count the paths someone
    /// remembered to instrument.
    pub fn canonicalize(&self, target: &Iri) -> Canonical {
        self.walk(target, true)
    }

    /// [`canonicalize`](Self::canonicalize) WITHOUT touching a counter: what the
    /// table would do with `target`, asked by an observer. `urn:kernel:explain` is
    /// that observer — a dry run of resolution must not count as a resolution, or
    /// asking why a name misses would itself move the `unresolved` count an operator
    /// reads at `urn:kernel:aliases`.
    pub(crate) fn preview(&self, target: &Iri) -> Canonical {
        self.walk(target, false)
    }

    /// [`canonicalize`](Self::canonicalize) as the calling thread's resolution asks
    /// for it: the counter-free [`preview`](Self::preview) inside [`previewing`], the
    /// counting walk everywhere else.
    fn canonicalize_here(&self, target: &Iri) -> Canonical {
        self.walk(target, !PREVIEWING.with(Cell::get))
    }

    /// The walk both faces share; `record` says whether it bumps the counters.
    fn walk(&self, target: &Iri, record: bool) -> Canonical {
        // The reserved namespace is not aliasable, in any case of its prefix (above).
        if crate::kernel::kernel_op(target.as_str()).is_some() {
            return Canonical::Direct;
        }
        let mut current = target.clone();
        let mut trail = vec![target.as_str().to_string()];
        let mut applied: Vec<usize> = Vec::new();
        for _ in 0..self.max_hops {
            let Some((index, next)) = self.step(current.as_str()) else {
                // No rule matches: the walk has terminated.
                return if applied.is_empty() {
                    Canonical::Direct
                } else {
                    if record {
                        for &index in &applied {
                            self.rules[index].hops.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Canonical::Aliased(AliasHop {
                        logical: target.clone(),
                        canonical: current,
                        rules: applied,
                    })
                };
            };
            applied.push(index);
            let Ok(next_iri) = Iri::parse(next.clone()) else {
                return self.refuse(
                    target,
                    format!("rewrite produced `{next}`, which is not a valid IRI"),
                    trail,
                    applied,
                    record,
                );
            };
            if trail.iter().any(|seen| seen == next_iri.as_str()) {
                trail.push(next_iri.as_str().to_string());
                return self.refuse(target, "alias cycle".to_string(), trail, applied, record);
            }
            trail.push(next_iri.as_str().to_string());
            current = next_iri;
        }
        self.refuse(
            target,
            format!("alias chain exceeded {} hops", self.max_hops),
            trail,
            applied,
            record,
        )
    }

    /// The most specific rule matching `target`, applied once.
    fn step(&self, target: &str) -> Option<(usize, String)> {
        self.rules
            .iter()
            .enumerate()
            .find_map(|(index, rule)| rule.apply(target).map(|next| (index, next)))
    }

    fn refuse(
        &self,
        logical: &Iri,
        reason: String,
        trail: Vec<String>,
        rules: Vec<usize>,
        record: bool,
    ) -> Canonical {
        if record {
            for &index in &rules {
                self.rules[index].refused.fetch_add(1, Ordering::Relaxed);
            }
        }
        Canonical::Refused(AliasRefusal {
            logical: logical.clone(),
            reason,
            trail,
            rules,
        })
    }

    /// Record that a rewritten target resolved to nothing. Called by the kernel on
    /// [`crate::Error::Unresolved`] so the miss is attributed to
    /// the rule that moved the name.
    ///
    /// A hop whose path does not match this table's current rules is ignored. Hops
    /// can outlive a builder insertion (which re-sorts rule indices), or come from
    /// another table; their indices alone are not enough to attribute a miss.
    pub fn record_unresolved(&self, hop: &AliasHop) {
        // Validate the whole path, not only its bounds: an in-range index may name
        // a different rule. Preview does not alter hops or refusal counters.
        if self.preview(hop.logical()).hop() != Some(hop) {
            return;
        }
        for &index in &hop.rules {
            self.rules[index].unresolved.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether any granted scope mentions a rewritten namespace — the hint that
    /// turns decision 1's silent denial into a sentence. A capability naming
    /// `urn:cap:fs:read:urn:example:fs:ws/x` while `urn:example:fs:` is being migrated is almost
    /// certainly a grant that was not migrated with the resource.
    ///
    /// Deliberately a *hint*, not a decision: nothing here widens or rewrites
    /// authority. It only reports a coincidence worth looking at, and the caller
    /// labels it as such.
    pub fn scopes_naming_stale_namespaces<'a, I>(&self, scopes: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a String>,
    {
        let mut hits = Vec::new();
        for scope in scopes {
            if self
                .rules
                .iter()
                .any(|rule| scope.contains(rule.from.as_str()))
            {
                hits.push(scope.clone());
            }
        }
        hits
    }

    /// Parse the line-oriented text form:
    ///
    /// ```text
    /// # the urn:iki: migration
    /// prefix  urn:fn:         urn:iki:fn:
    /// exact   urn:log:config  file:/logConfig.yaml
    /// exact   urn:example:view:open  urn:example:ledger:open  root
    /// max-hops 8
    /// ```
    ///
    /// An optional fourth field says where the rewritten name resolves: `child` (the
    /// default, and never written back) or `root` ([`ResolveAt`]). A `template` rule (a
    /// named request whose logical name is a URI template, ledger #2512) is always `root`.
    ///
    /// **A rule with arguments has no line form**: its fields cannot hold a quoted value,
    /// so a line carrying `key=value` words is refused, naming the two forms that can
    /// write one — a Turtle declaration (`ik:fixes`) and a host's names file.
    ///
    /// ```
    /// use ikigai_core::AliasTable;
    ///
    /// let refused = AliasTable::parse("exact urn:example:count urn:example:sparql root graph=urn:g")
    ///     .unwrap_err();
    /// assert!(refused.detail.contains("ik:fixes"), "{refused}");
    /// let template = AliasTable::parse("template urn:example:e:{id} urn:example:page:{id} root").unwrap();
    /// assert_eq!(template.rules()[0].match_kind().keyword(), "template");
    /// ```
    ///
    /// Blank lines and `#` comments are ignored. Whitespace-separated, so neither
    /// field may contain a space — IRIs may not either, so nothing is lost.
    pub fn parse(text: &str) -> Result<AliasTable, AliasParseError> {
        let mut table = AliasTable::new();
        for (offset, raw) in text.lines().enumerate() {
            let line = offset + 1;
            let content = raw.split('#').next().unwrap_or("").trim();
            if content.is_empty() {
                continue;
            }
            let fields: Vec<&str> = content.split_whitespace().collect();
            match fields.as_slice() {
                ["max-hops", hops] => {
                    let hops = hops.parse::<usize>().map_err(|_| AliasParseError {
                        line,
                        detail: format!("`{hops}` is not a hop count"),
                    })?;
                    table = table.with_max_hops(hops);
                }
                [kind @ ("exact" | "prefix" | "template"), _, _, rest @ ..]
                    if rest.iter().any(|field| field.contains('=')) =>
                {
                    return Err(AliasParseError {
                        line,
                        detail: format!(
                            "`{content}` is a {kind} rule with arguments, which the line form \
                             cannot write (its fields cannot hold a quoted value): declare it \
                             as a named request in Turtle (ik:fixes on an ik:RewriteRule) or in \
                             a host's names file"
                        ),
                    })
                }
                ["template", from, to, rest @ ..] => {
                    let at = match rest {
                        ["root"] => ResolveAt::Root,
                        _ => {
                            return Err(AliasParseError {
                                line,
                                detail: format!(
                                    "a template rule is a named request and resolves from the \
                                     root: write `template {from} {to} root`"
                                ),
                            })
                        }
                    };
                    let rule = TopologyRule::new(RuleKind::Exact, *from, *to).resolving_at(at);
                    let mut rule = rule;
                    rule.template = true;
                    table = table.with_rule(rule).map_err(|e| AliasParseError {
                        line,
                        detail: e.to_string(),
                    })?;
                }
                [kind @ ("exact" | "prefix"), from, to, rest @ ..] if rest.len() <= 1 => {
                    let at = match rest {
                        [] | ["child"] => ResolveAt::Child,
                        ["root"] => ResolveAt::Root,
                        [other, ..] => {
                            return Err(AliasParseError {
                                line,
                                detail: format!(
                                    "`{other}` is not where a rule resolves (child or root)"
                                ),
                            })
                        }
                    };
                    // Both sides must be IRIs (a prefix is one too: `urn:fn:`
                    // parses). Validating here means a malformed table fails when
                    // it is read, not on the one request that happens to hit the
                    // bad rule.
                    for (label, value) in [("from", from), ("to", to)] {
                        Iri::parse(*value).map_err(|e| AliasParseError {
                            line,
                            detail: format!("{label} `{value}`: {e}"),
                        })?;
                    }
                    let kind = if *kind == "exact" {
                        RuleKind::Exact
                    } else {
                        RuleKind::Prefix
                    };
                    table = table.rule_at(kind, *from, *to, at);
                }
                _ => {
                    return Err(AliasParseError {
                        line,
                        detail: format!(
                            "expected `exact <from> <to> [root]`, `prefix <from> <to> [root]`, \
                             `template <from> <to> root` or `max-hops <n>`, got `{content}`"
                        ),
                    })
                }
            }
        }
        Ok(table)
    }

    /// Render the table back to its text form — round-trips through
    /// [`parse`](AliasTable::parse) for every rule without arguments. Counters are *not*
    /// rendered: this is the table's definition, not its readout (that is
    /// `urn:kernel:aliases`). A named request with arguments is rendered with them as
    /// `key=value` words, which `parse` refuses by name rather than reading half of.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if self.max_hops != DEFAULT_MAX_HOPS {
            out.push_str(&format!("max-hops {}\n", self.max_hops));
        }
        for rule in &self.rules {
            out.push_str(&format!(
                "{} {} {}",
                rule.match_kind().keyword(),
                rule.from,
                rule.to
            ));
            if rule.at != ResolveAt::Child {
                out.push_str(&format!(" {}", rule.at.keyword()));
            }
            // A named request's arguments, as words [`parse`](Self::parse) REFUSES by
            // name: a readout of the rule, never a silent half of it on the way back.
            if let Some(verb) = rule.request.verb {
                out.push_str(&format!(" verb={verb:?}"));
            }
            for fixed in &rule.request.fixes {
                out.push_str(&format!(" {}={}", fixed.name, fixed.value));
            }
            for input in &rule.request.inputs {
                out.push_str(&format!(" input={}", input.name));
            }
            out.push('\n');
        }
        out
    }
}

thread_local! {
    /// Set while [`previewing`] runs a resolution on this thread.
    static PREVIEWING: Cell<bool> = const { Cell::new(false) };
}

/// Run `resolve` as an observer's resolution: every [`Alias`] it reaches rewrites
/// through the counter-free [`AliasTable::preview`], so asking what a name resolves
/// to never counts as resolving it at `urn:kernel:aliases`. The cache-status probe
/// ([`Kernel::is_cached_in`](crate::Kernel::is_cached_in)) is that observer: it must
/// learn the canonical an overlay reports (decision 3) without a hop per probe
/// (ledger #2503). Resolution is synchronous, so a thread-local reaches every
/// overlay on the walk without widening `Space::resolve` or the one-word `Scope`.
pub(crate) fn previewing<R>(resolve: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PREVIEWING.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(PREVIEWING.with(|flag| flag.replace(true)));
    resolve()
}

/// A `Space` decorator that rewrites logical names to backing ones before
/// delegating — the interception-overlay form of [`AliasTable`].
///
/// Composing it by hand is fine, including underneath another overlay: the
/// rewrite is reported on [`Resolved::canonical`](crate::Resolved::canonical) and
/// the kernel adopts it, so the logical and backing names share a cache entry and
/// a golden thread either way (decision 3). What
/// [`Kernel::with_aliases`](crate::Kernel::with_aliases) adds on top is the
/// operator's half: the live readout at `urn:kernel:aliases`, refusal of a cyclic
/// or over-long chain *before* dispatch rather than at invoke, and per-rule
/// attribution of a rewrite that landed on nothing.
///
/// One thing only the kernel-held table can do: rewrite a name *into* the reserved
/// `urn:kernel:` namespace. The kernel dispatches its own builtins ahead of the
/// root space, so a rewrite performed inside the root space arrives too late — the
/// backing name is looked for in the space, where no builtin is bound, and misses.
/// Aliasing one *away* is refused outright, by both routes.
///
/// Resolving an already-canonical target through this space is a no-op: the rules
/// do not match their own destinations in a well-formed table, so the kernel's
/// canonicalize-then-resolve is idempotent and costs one failed scan.
///
/// # Where the rewritten name resolves
///
/// **An `Alias` space resolves a rewritten name only in its own child** — the space it
/// wraps — for every rule of the default kind ([`ResolveAt::Child`]). It never reaches
/// the rest of the arrangement: an `Alias` placed inside a dynamic space whose rule
/// names something the HOST serves elsewhere misses, because the host's doors are not
/// in its child (ledger #2061). Only [`Kernel::with_aliases`](crate::Kernel::with_aliases)
/// rewrites BEFORE root resolution, and that table is the host's, fixed at build.
///
/// A rule declared [`ResolveAt::Root`] ([`AliasTable::exact_at_root`],
/// [`AliasTable::prefix_at_root`], `ik:resolveAt "root"` in a declaration) is the
/// other half: a **view**. A name it matches resolves to a forwarding endpoint, and
/// invoking that endpoint issues the rewritten name as a sub-request from the root of
/// the caller's resolution chain ([`Invocation::issue`](crate::Invocation::issue)), with
/// the same verb and arguments, and answers what that sub-request answers. When any
/// rule of a chain is a root rule, the chain's final name is resolved from the root.
///
/// - **It grants nothing.** The sub-request runs under the caller's own capability,
///   so a caller who may not read the target is refused exactly as if they had asked
///   for it directly; the view's own door declares no capability of its own.
/// - **It never stands in for an existing name.** The space is consulted where it is
///   placed, so a name an earlier layer answers never reaches it, and it forwards only
///   names its own rules match.
/// - **The view's answer is its OWN cache entry, depending on the target's.** No
///   canonical is reported (reporting one would key the view under the target, and the
///   forwarder's own sub-request would then be the entry it is computing). The
///   sub-request's dependencies propagate, so cutting the target's thread invalidates
///   the view, and the view expires no later than the target; inside a
///   [`Threaded`](crate::Threaded) or [`DynamicSpace`](crate::DynamicSpace) the answer
///   also hangs from the space's thread, so swapping the declaration invalidates it.
/// - **Cycles terminate.** A cycle inside one table is refused by the table walk (and
///   by [`build`](crate::build) before it binds); a cycle through two tables (a view
///   onto a view onto the first) is stopped by the kernel's nesting-depth bound
///   ([`Kernel::with_max_depth`](crate::Kernel::with_max_depth)) as
///   [`Error::DepthExceeded`].
/// - **`Meta` of a view is its TARGET's contract** (ledger #2513): the view says what it
///   forwards to ([`Endpoint::forwards`]), and the kernel follows that wherever it
///   describes anything — `Meta`, the catalog, `urn:kernel:actions` — so a view is listed
///   under its own name with the target's inputs (a named request's fixed ones listed and
///   marked), outputs and `requires`. Only a view whose target resolves to nothing shows
///   the forwarder's own contract, whose argument set is open (every argument passes
///   through) and whose summary says the target is missing.
///
/// # Named requests
///
/// A view may bind its name to a request WITH ARGUMENTS (ledger #2512): a rule added with
/// [`AliasTable::with_rule`] from a [`TopologyRule::named_request`] or
/// [`TopologyRule::named_template`], or declared in Turtle (`ik:fixes`, `ik:verb`,
/// `ik:input`, `ik:ruleKind "template"`). It is partial application, merged into the
/// forwarded request in the order the chain applied its rules:
///
/// - **a fixed argument is the binding's**: a caller who gives one is refused with
///   [`Error::InvalidArgument`] naming the binding (`fixed by <urn:…> to …`), never
///   silently overridden and never silently ignored;
/// - **every other argument passes through** to the target unchanged;
/// - **a declared input** is the caller's to give, and its default is filled when the
///   caller leaves it out; `as` is never fixed, and a binding's default face is a
///   declared `as` with a default, so a caller's `as=` still negotiates;
/// - **a bound verb** is the one verb forwarded, beside `Exists`; any other is refused
///   with [`Error::Unsupported`];
/// - **a template rule's captures** may fill its canonical name and whole argument
///   values ([`FixedValue::Capture`]), or encoded reference IRIs
///   ([`FixedValue::ReferenceTemplate`]).
///
/// A bound name's answer is its own cache entry depending on the expansion's, like any
/// view's; the expansion is the ordinary request, shared with anyone who asks for it.
///
/// ```
/// use std::sync::Arc;
/// use futures::executor::block_on;
/// use ikigai_core::{
///     Alias, AliasTable, Capability, EndpointSpace, Exact, Fallback, FnEndpoint, Iri, Kernel,
///     ReprType, Representation, Request, Space, Verb,
/// };
///
/// let open = FnEndpoint::new("open", |_| {
///     Ok(Representation::new(ReprType::new("text/plain"), b"3 open".to_vec()).cacheable())
/// });
/// let host = EndpointSpace::new().bind(Exact::new("urn:example:ledger:open"), open);
/// let views = Alias::new(
///     Arc::new(AliasTable::new().exact_at_root("urn:example:view:open", "urn:example:ledger:open")),
///     Arc::new(EndpointSpace::new()),
/// );
/// let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(host), Arc::new(views)]));
/// let kernel = Kernel::new(root);
/// let view = Request::new(Verb::Source, Iri::parse("urn:example:view:open").unwrap());
/// let answer = block_on(kernel.issue(view, &Capability::root())).unwrap();
/// assert_eq!(answer.bytes, b"3 open");
/// ```
pub struct Alias {
    table: Arc<AliasTable>,
    inner: Arc<dyn Space>,
    id: Option<Iri>,
}

impl Alias {
    /// Wrap `inner`, rewriting through `table`.
    pub fn new(table: Arc<AliasTable>, inner: Arc<dyn Space>) -> Self {
        Alias {
            table,
            inner,
            id: None,
        }
    }

    /// Claim an identity for this overlay (builder) — a claim, same name ⇒ same
    /// doors; see [`EndpointSpace::named`](crate::EndpointSpace::named).
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
    }

    /// The table this overlay rewrites through.
    pub fn table(&self) -> &Arc<AliasTable> {
        &self.table
    }
}

impl Space for Alias {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        // The innermost named space answered; this overlay only fills an absence.
        answered(self.resolve_through(request, scope), &self.id)
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        // Decision 4: transparent to enumeration for a renaming. The catalog and the
        // action manifold see the BACKING names, once each; the aliases are their own
        // resource at `urn:kernel:aliases`. A ROOT rule is not a renaming but a name of its
        // own — a view, or a named request — so each exact and template root rule is listed
        // (ledger #2513), and a walk describes it by its target's contract. A root PREFIX
        // rule names a family no pattern lists, so it is not.
        let mut entries = self.inner.entries()?;
        for rule in &self.table.rules {
            if rule.at == ResolveAt::Root && rule.match_kind() != MatchKind::Prefix {
                entries.push(SpaceEntry::new(rule.from.clone(), "alias-view"));
            }
        }
        Some(entries)
    }

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    /// The table is data, so the node carries it: one `ik:RewriteRule` per rule,
    /// in table order — the SORTED precedence order (see [`AliasTable`]), the first
    /// rule that matches a name applying — the preimage a reachability query needs
    /// to see.
    fn topology(&self) -> Topology {
        Topology::new(SpaceKind::Alias {
            rules: self.table.rules().iter().map(AliasRule::summary).collect(),
            max_hops: self.table.max_hops(),
        })
        .with_id(self.id.clone())
        .child(self.inner.topology())
    }
}

impl Alias {
    fn resolve_through(&self, request: &Request, scope: &Scope) -> Resolution {
        match self.table.canonicalize_here(&request.target) {
            Canonical::Direct => self.inner.resolve(request, scope),
            Canonical::Aliased(hop)
                if hop
                    .rules()
                    .iter()
                    .any(|&i| self.table.rules()[i].resolve_at() == ResolveAt::Root) =>
            {
                // A view: forward from the root under the caller's capability. No
                // canonical is reported — the view's answer is its own entry, hung from
                // the target's threads by the sub-request (see the type's docs). A named
                // request's parts ride along, replayed from the chain the walk took.
                let endpoint: Arc<dyn Endpoint> = match self.table.view(&request.target, &hop) {
                    Some(view) => Arc::new(view),
                    None => refused_endpoint(&AliasRefusal {
                        logical: request.target.clone(),
                        reason: "a named request's chain did not replay".into(),
                        trail: vec![request.target.as_str().to_string()],
                        rules: hop.rules().to_vec(),
                    }),
                };
                Resolution::Hit(Resolved::new(endpoint, Bindings::default()))
            }
            Canonical::Aliased(hop) => {
                let mut rewritten = request.clone();
                rewritten.target = hop.canonical().clone();
                // ★ REPORT THE REWRITE. This is what makes decision 3 hold for an
                // `Alias` the kernel did not install itself: the canonical rides
                // back on the `Resolved`, and the kernel adopts it before it
                // computes the cache key or fires the auto-cut. A space nested
                // deeper that rewrote further has already named the resource
                // actually reached, so `with_canonical` keeps that one.
                match self.inner.resolve(&rewritten, scope) {
                    Resolution::Hit(hit) => {
                        Resolution::Hit(hit.with_canonical(hop.canonical().clone()))
                    }
                    Resolution::Miss => Resolution::Miss,
                }
            }
            // A `Space` cannot return an error, and a refusal must not read as a
            // miss (a miss says "nothing is bound there", which is a different
            // and misleading fact). So it resolves to an endpoint that fails on
            // invoke with the trail — the same idiom `RateLimit` uses for an
            // over-budget request. A kernel refuses earlier, before dispatch.
            // No canonical is reported: a refusal resolved to nothing, and a
            // half-applied rewrite is exactly what decision 5 forbids.
            Canonical::Refused(refusal) => Resolution::Hit(Resolved::new(
                refused_endpoint(&refusal),
                Bindings::default(),
            )),
        }
    }
}

/// The endpoint a root-resolving rule's match resolves to: it re-issues the request
/// under the rewritten name, from the root, under the caller's own capability — with a
/// named request's arguments merged in (ledger #2512).
struct ViewEndpoint {
    target: Iri,
    /// Each named request the chain passed through, in the order it applied.
    parts: Vec<BoundPart>,
    /// What it forwards to, as the kernel describes it (ledger #2513).
    forward: crate::Forward,
}

/// One named request of a view's chain, with its captures settled.
struct BoundPart {
    /// The name the rule matched (for a template rule, the concrete name).
    name: Iri,
    verb: Option<Verb>,
    /// Each fixed argument: its name, the value sent, and how a refusal shows it.
    fixed: Vec<(String, Result<ArgRef, String>, String)>,
    inputs: Vec<ArgSpec>,
}

impl AliasTable {
    /// The view a root chain resolves to: the chain's final name, and each named request
    /// it passed through, replayed from `logical` so a template rule's captures are known.
    fn view(&self, logical: &Iri, hop: &AliasHop) -> Option<ViewEndpoint> {
        let mut current = logical.clone();
        let mut parts = Vec::new();
        // The name the caller's name is bound under: the first rule's logical name (a
        // template, for a template rule, whose captures the caller supplies).
        let first = self.rules.get(*hop.rules().first()?)?;
        let mut forward = crate::Forward::new(first.from.clone(), hop.canonical().clone());
        if let Some(template) = &first.request.template {
            for var in template.variables() {
                forward = forward.capturing(var);
            }
        }
        for (place, &index) in hop.rules().iter().enumerate() {
            let rule = self.rules.get(index)?;
            let (next, captures) = rule.apply_capturing(current.as_str())?;
            if rule.is_named_request() {
                if forward.verb.is_none() {
                    forward.verb = rule.request.verb;
                }
                for argument in &rule.request.fixes {
                    // A later rule's capture is of a name the caller never wrote: it is a
                    // value by then.
                    let argument = match (&argument.value, place) {
                        (FixedValue::Capture(var), 1..) => FixedArgument::value(
                            argument.name.clone(),
                            captures.get(var).unwrap_or_default(),
                        ),
                        (FixedValue::ReferenceTemplate(template), 1..) => {
                            match reference_from_template(template, &captures) {
                                Ok(iri) => FixedArgument::reference(argument.name.clone(), iri),
                                // The invoke path reports the invalid capture by argument
                                // name; keep the authored template in its description.
                                Err(_) => argument.clone(),
                            }
                        }
                        _ => argument.clone(),
                    };
                    forward = forward.fixing(argument);
                }
                for input in &rule.request.inputs {
                    if !forward.inputs.iter().any(|i| i.name == input.name) {
                        forward = forward.input(input.clone());
                    }
                }
                let mut fixed = Vec::with_capacity(rule.request.fixes.len());
                for argument in &rule.request.fixes {
                    let (value, shown) = match &argument.value {
                        FixedValue::Value(value) => (
                            Ok(ArgRef::Inline(value.clone().into_bytes())),
                            value.clone(),
                        ),
                        FixedValue::Reference(iri) => (
                            Ok(ArgRef::Reference(iri.clone())),
                            format!("<{}>", iri.as_str()),
                        ),
                        FixedValue::ReferenceTemplate(template) => (
                            reference_from_template(template, &captures).map(ArgRef::Reference),
                            argument.value.to_string(),
                        ),
                        FixedValue::Capture(var) => {
                            let captured = captures.get(var)?.to_string();
                            (Ok(ArgRef::Inline(captured.clone().into_bytes())), captured)
                        }
                    };
                    fixed.push((argument.name.clone(), value, shown));
                }
                parts.push(BoundPart {
                    name: current.clone(),
                    verb: rule.request.verb,
                    fixed,
                    inputs: rule.request.inputs.clone(),
                });
            }
            current = Iri::parse(next).ok()?;
        }
        (current == *hop.canonical()).then_some(ViewEndpoint {
            target: current,
            parts,
            forward,
        })
    }
}

/// Bind-time validation guarantees the shape; runtime validation guards captured data.
fn reference_from_template(template: &UriTemplate, captures: &Bindings) -> Result<Iri, String> {
    for var in template.variables() {
        if captures.get(var).is_none_or(str::is_empty) {
            return Err(format!("reference template capture {{{var}}} is empty"));
        }
    }
    // Parse only the structure here: braces in the authored template stand for
    // variables, and bind-time validation has checked the same structure with samples.
    // Inspect only path segments, so a dot in a query or a fragment stays legal.
    let structure = oxiri::Iri::parse_unchecked(template.source());
    for segment in structure.path().split('/') {
        if segment.contains('{') {
            let segment = UriTemplate::parse(segment).map_err(|e| e.to_string())?;
            if matches!(
                segment.expand_encoded(captures).as_deref(),
                Some("." | "..")
            ) {
                return Err("reference template capture forms a dot path segment".into());
            }
        }
    }
    let expanded = template
        .expand_encoded(captures)
        .ok_or_else(|| "reference template capture is missing".to_string())?;
    Iri::parse(expanded).map_err(|e| format!("expanded reference template is not an IRI: {e}"))
}

impl ViewEndpoint {
    /// The request this view issues for `inv`: the caller's, at the target, with every
    /// fixed argument merged in and every declared default the caller left out.
    ///
    /// **Fixed wins, and says so** (ledger #2512): a caller who gives a fixed argument is
    /// REFUSED, naming the binding, never silently overridden (that would make the name
    /// mean any request) and never silently ignored. Every other argument passes through.
    fn forwarded(&self, inv: &Invocation<'_>) -> crate::error::Result<Request> {
        let mut forwarded = inv.request.clone();
        forwarded.target = self.target.clone();
        for part in &self.parts {
            if let Some(bound) = part.verb {
                let verb = inv.request.verb;
                if verb != bound && verb != Verb::Exists {
                    return Err(Error::Unsupported {
                        verb: Some(verb),
                        detail: format!(
                            "<{}> is a named request for {bound:?}: it answers {bound:?}, \
                             Exists and Meta",
                            part.name.as_str()
                        ),
                    });
                }
            }
            for (name, value, shown) in &part.fixed {
                if forwarded.args.contains_key(name) {
                    return Err(Error::InvalidArgument {
                        name: name.clone(),
                        detail: format!("fixed by <{}> to {shown}", part.name.as_str()),
                    });
                }
                let value = value.as_ref().map_err(|detail| Error::InvalidArgument {
                    name: name.clone(),
                    detail: detail.clone(),
                })?;
                forwarded.args.insert(name.clone(), value.clone());
            }
            for input in &part.inputs {
                if let Some(default) = &input.default {
                    forwarded
                        .args
                        .entry(input.name.clone())
                        .or_insert_with(|| ArgRef::Inline(default.clone().into_bytes()));
                }
            }
        }
        Ok(forwarded)
    }

    /// The one verb the chain binds, if any.
    fn bound_verb(&self) -> Option<Verb> {
        self.parts.iter().find_map(|part| part.verb)
    }
}

#[async_trait::async_trait]
impl Endpoint for ViewEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> crate::error::Result<Representation> {
        // `issue`, never an attenuating or widening form: the caller's capability, as
        // it arrived. Its dependencies are recorded, so the view hangs from the target.
        inv.issue(self.forwarded(inv)?).await
    }

    /// What the kernel calls: the target's answer as the target produced it, a stream
    /// included, never collected on the way through the view (ledger #2512). A body the
    /// caller attached rides the same run, so the target reads it.
    async fn invoke_body(
        &self,
        inv: &Invocation<'_>,
    ) -> crate::error::Result<Representation<crate::Body>> {
        inv.issue_body(self.forwarded(inv)?).await
    }

    fn name(&self) -> &str {
        "alias-view"
    }

    fn forwards(&self) -> Option<crate::Forward> {
        Some(self.forward.clone())
    }

    fn describe(&self) -> Description {
        let mut summary = format!("Forwards to <{}>", self.target.as_str());
        let fixed: Vec<String> = self
            .parts
            .iter()
            .flat_map(|part| &part.fixed)
            .map(|(name, _, shown)| format!("{name}={shown}"))
            .collect();
        if !fixed.is_empty() {
            summary.push_str(&format!(" with {}", fixed.join(" ")));
        }
        summary.push_str(
            ", resolved from the root under the caller's own capability (an \
             ik:RewriteRule with ik:resolveAt \"root\").",
        );
        let mut description = Description::new("alias-view")
            .title(if self.parts.is_empty() {
                "A view onto another name"
            } else {
                "A named request"
            })
            .summary(summary)
            // Every argument a caller gives passes through to the target, so the set is
            // open: an engine names each `key=value` rather than routing it as a value.
            .open_arguments("an argument of the target, passed through unchanged");
        description = match self.bound_verb() {
            Some(verb) => description.verb(verb).verb(Verb::Exists).verb(Verb::Meta),
            None => description
                .verb(Verb::Source)
                .verb(Verb::Sink)
                .verb(Verb::Exists)
                .verb(Verb::Delete)
                .verb(Verb::Meta),
        };
        let mut declared: Vec<&str> = Vec::new();
        for input in self.parts.iter().flat_map(|part| &part.inputs) {
            if !declared.contains(&input.name.as_str()) {
                declared.push(&input.name);
                description = description.input(input.clone());
            }
        }
        description
    }
}

/// Whether `rule` can be added to a table: the refusal [`AliasTable::with_rule`] gives,
/// without building anything.
pub(crate) fn check_rule(rule: &TopologyRule) -> Result<(), DeclarationError> {
    request_part(rule).map(|_| ())
}

/// The named-request part of `rule`, refused when the rule cannot mean one thing (see
/// [`AliasTable::with_rule`]).
fn request_part(rule: &TopologyRule) -> Result<RequestPart, DeclarationError> {
    let refuse = |reason: String| DeclarationError::Malformed {
        node: Some(rule.from.clone()),
        reason,
    };
    let named =
        rule.template || rule.verb.is_some() || !rule.fixes.is_empty() || !rule.inputs.is_empty();
    if !named {
        return Ok(RequestPart::default());
    }
    if rule.at != ResolveAt::Root {
        return Err(refuse(
            "a named request (a template, a bound verb, fixed arguments or declared inputs) \
             resolves from the root: mark it ik:resolveAt \"root\""
                .into(),
        ));
    }
    let pattern = |text: &str| {
        UriTemplate::parse(text).map_err(|e| DeclarationError::Pattern {
            node: rule.from.clone(),
            pattern: text.to_string(),
            reason: format!("is not a URI template: {e}"),
        })
    };
    let template = if rule.template {
        if rule.kind != RuleKind::Exact {
            return Err(refuse(
                "a template rule matches one whole name: it is never a prefix rule".into(),
            ));
        }
        let template = pattern(&rule.from)?;
        if template.variables().next().is_none() {
            return Err(refuse(
                "a template rule captures at least one {var}; with none it is an exact rule".into(),
            ));
        }
        Some(template)
    } else {
        None
    };
    let vars: Vec<String> = template
        .as_ref()
        .map(|t| t.variables().map(str::to_string).collect())
        .unwrap_or_default();
    let target = if rule.to.contains('{') {
        let target = pattern(&rule.to)?;
        if let Some(var) = target.variables().find(|v| !vars.iter().any(|w| w == v)) {
            return Err(refuse(format!(
                "its canonical name uses {{{var}}}, which its logical name does not capture"
            )));
        }
        Some(target)
    } else {
        None
    };
    if rule.verb == Some(Verb::Meta) {
        return Err(refuse(
            "a named request forwards Source, Sink, Exists or Delete: its Meta is answered \
             from its contract"
                .into(),
        ));
    }
    let named_ok = |name: &str, what: &str| {
        if name.is_empty() || !crate::iri::is_iri_safe(name) {
            Err(refuse(format!(
                "{what} `{name}` is not a name a declaration can state (empty, or not IRI-safe)"
            )))
        } else {
            Ok(())
        }
    };
    let mut names: Vec<&str> = Vec::new();
    for argument in &rule.fixes {
        named_ok(&argument.name, "it fixes")?;
        if argument.name == "as" {
            return Err(refuse(
                "it fixes `as`, which negotiates the face and is never fixed: declare `as` as \
                 an input with a default, the name's default face"
                    .into(),
            ));
        }
        if names.contains(&argument.name.as_str()) {
            return Err(refuse(format!("it fixes `{}` twice", argument.name)));
        }
        names.push(&argument.name);
        if let FixedValue::ReferenceTemplate(template) = &argument.value {
            let mut samples = Bindings::new();
            for var in template.variables() {
                if !vars.iter().any(|v| v == var) {
                    return Err(refuse(format!(
                        "it fixes `{}` with unknown capture {{{var}}}",
                        argument.name
                    )));
                }
                samples.insert(var, "segment");
            }
            if samples.is_empty() {
                return Err(refuse(format!(
                    "reference template for `{}` needs at least one variable; use a plain reference",
                    argument.name
                )));
            }
            Iri::parse(
                template
                    .expand_encoded(&samples)
                    .expect("all variables supplied"),
            )
            .map_err(|e| {
                refuse(format!(
                    "reference template for `{}` is not an IRI: {e}",
                    argument.name
                ))
            })?;
        }
        if let FixedValue::Capture(var) = &argument.value {
            if !vars.iter().any(|v| v == var) {
                return Err(refuse(format!(
                    "it fixes `{}` to the capture {{{var}}}, which its logical name does not have",
                    argument.name
                )));
            }
        }
    }
    for input in &rule.inputs {
        named_ok(&input.name, "it declares")?;
        if names.contains(&input.name.as_str()) {
            return Err(refuse(format!(
                "it both fixes and declares `{}`: a declared input is the caller's to give",
                input.name
            )));
        }
        names.push(&input.name);
        if input.source == InputSource::Binding {
            return Err(refuse(format!(
                "it declares `{}` as a binding: a template rule's captures are its bindings",
                input.name
            )));
        }
        if !declarable(input) {
            return Err(refuse(format!(
                "it declares `{}` with more than a declaration states (an input's name, \
                 summary, required, class, default and one_of)",
                input.name
            )));
        }
    }
    let mut fixes = rule.fixes.clone();
    fixes.sort_by(|a, b| a.name.cmp(&b.name));
    let mut inputs = rule.inputs.clone();
    inputs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(RequestPart {
        template,
        target,
        verb: rule.verb,
        fixes,
        inputs,
    })
}

/// Whether a declared input carries only what a declaration's input node states: its
/// name, summary, required, class, default and enumeration. Anything else would be lost
/// on the way through Turtle, and a declaration never skips.
pub(crate) fn declarable(input: &ArgSpec) -> bool {
    input.examples.is_empty()
        && input.needs.is_empty()
        && input.list.is_none()
        && input.values.is_empty()
        && !input.reads_body
        && input.fixed.is_none()
        && match input.class.as_deref() {
            Some(class) => crate::iri::is_iri_safe(class),
            None => true,
        }
}

/// The endpoint a refused rewrite resolves to: it errors on invoke with the trail.
fn refused_endpoint(refusal: &AliasRefusal) -> Arc<dyn Endpoint> {
    let message = format!("alias: {refusal}");
    let summary = message.clone();
    Arc::new(
        FnEndpoint::new("alias-refused", move |_inv| {
            Err(Error::Endpoint(message.clone()))
        })
        .with_description(
            Description::new("alias-refused")
                .title("Alias rewrite refused")
                .summary(summary)
                .verb(Verb::Source)
                .verb(Verb::Meta)
                .output("text/plain;charset=utf-8"),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;
    use crate::grammar::Exact;
    use crate::space::EndpointSpace;

    fn iri(s: &str) -> Iri {
        Iri::parse(s).unwrap()
    }

    #[test]
    fn reference_template_capture_safety() {
        let template = UriTemplate::parse("urn:example:files/searches/{file}").unwrap();
        for (capture, encoded) in [
            ("a/b", "a%2Fb"),
            ("../x", "..%2Fx"),
            ("x?y", "x%3Fy"),
            ("x#y", "x%23y"),
            ("50%", "50%25"),
            ("a b", "a%20b"),
            ("ü", "%C3%BC"),
            ("a:b", "a%3Ab"),
            ("a@b", "a%40b"),
        ] {
            let mut captures = Bindings::new();
            captures.insert("file", capture);
            assert_eq!(
                reference_from_template(&template, &captures)
                    .unwrap()
                    .as_str(),
                format!("urn:example:files/searches/{encoded}")
            );
        }
        for capture in [".", "..", ""] {
            let mut captures = Bindings::new();
            captures.insert("file", capture);
            assert!(reference_from_template(&template, &captures).is_err());
        }
        let mut captures = Bindings::new();
        captures.insert("file", ".");
        // A dot is legal data unless it would form a whole path segment.
        for source in [
            "urn:example:files/{file}.rq",
            "https://example.test/?q={file}",
            "https://example.test/#{file}",
        ] {
            assert!(
                reference_from_template(&UriTemplate::parse(source).unwrap(), &captures).is_ok()
            );
        }
        // A partial capture can also complete a dot segment.
        assert!(reference_from_template(
            &UriTemplate::parse("https://example.test/.{file}").unwrap(),
            &captures
        )
        .is_err());
    }

    #[test]
    fn a_prefix_rule_carries_the_remainder_over() {
        let table = AliasTable::new().prefix("urn:fn:", "urn:iki:fn:");
        let canonical = table.canonicalize(&iri("urn:fn:toUpper"));
        assert_eq!(
            canonical.canonical().map(Iri::as_str),
            Some("urn:iki:fn:toUpper")
        );
        assert_eq!(table.rules()[0].hops(), 1);
    }

    #[test]
    fn an_exact_rule_rewrites_only_the_whole_name() {
        let table = AliasTable::new().exact("urn:log:config", "file:/logConfig.yaml");
        assert_eq!(
            table
                .canonicalize(&iri("urn:log:config"))
                .canonical()
                .map(Iri::as_str),
            Some("file:/logConfig.yaml")
        );
        // Not a prefix: a longer name under it is untouched.
        assert_eq!(
            table.canonicalize(&iri("urn:log:config:extra")),
            Canonical::Direct
        );
    }

    #[test]
    fn the_most_specific_rule_wins_regardless_of_declaration_order() {
        let table = AliasTable::new()
            .prefix("urn:fn:", "urn:iki:fn:")
            .exact("urn:fn:toUpper", "urn:legacy:upper");
        assert_eq!(
            table
                .canonicalize(&iri("urn:fn:toUpper"))
                .canonical()
                .map(Iri::as_str),
            Some("urn:legacy:upper")
        );
        assert_eq!(
            table
                .canonicalize(&iri("urn:fn:reverseList"))
                .canonical()
                .map(Iri::as_str),
            Some("urn:iki:fn:reverseList")
        );
    }

    #[test]
    fn a_chain_resolves_transitively() {
        // Decision 5, first half: A -> B -> C resolves.
        let table = AliasTable::new()
            .prefix("urn:a:", "urn:b:")
            .prefix("urn:b:", "urn:c:");
        let canonical = table.canonicalize(&iri("urn:a:thing"));
        assert_eq!(canonical.canonical().map(Iri::as_str), Some("urn:c:thing"));
        assert_eq!(canonical.hop().unwrap().rules().len(), 2);
    }

    #[test]
    fn a_cycle_is_refused_not_truncated() {
        // Decision 5, second half: A -> B, B -> A terminates with a refusal that
        // names the loop — never a partial rewrite to whichever name it stopped on.
        let table = AliasTable::new()
            .prefix("urn:a:", "urn:b:")
            .prefix("urn:b:", "urn:a:");
        let Canonical::Refused(refusal) = table.canonicalize(&iri("urn:a:thing")) else {
            panic!("a cycle must be refused");
        };
        assert_eq!(refusal.reason, "alias cycle");
        assert_eq!(refusal.trail, ["urn:a:thing", "urn:b:thing", "urn:a:thing"]);
        assert!(table.rules().iter().any(|rule| rule.refused() > 0));
    }

    #[test]
    fn an_over_long_chain_is_refused() {
        // A self-extending rule never revisits a name, so the visited set cannot
        // catch it — the hop budget is what stops it.
        let table = AliasTable::new()
            .prefix("urn:a:", "urn:a:x")
            .with_max_hops(3);
        let Canonical::Refused(refusal) = table.canonicalize(&iri("urn:a:thing")) else {
            panic!("an unbounded chain must be refused");
        };
        assert!(refusal.reason.contains("exceeded 3 hops"), "{refusal}");
    }

    #[test]
    fn the_kernel_namespace_is_not_aliasable() {
        let table = AliasTable::new().prefix("urn:kernel:", "urn:example:moved:kernel:");
        assert_eq!(
            table.canonicalize(&iri("urn:kernel:cut")),
            Canonical::Direct
        );
    }

    #[test]
    fn an_unmatched_name_costs_nothing_and_reports_direct() {
        let table = AliasTable::new().prefix("urn:fn:", "urn:iki:fn:");
        assert_eq!(table.canonicalize(&iri("urn:text:wc")), Canonical::Direct);
        assert_eq!(table.rules()[0].hops(), 0);
    }

    #[test]
    fn the_table_round_trips_through_its_text_form() {
        let text = "# comment\nprefix urn:fn: urn:iki:fn:\nexact urn:log:config file:/logConfig.yaml\nmax-hops 3\n";
        let table = AliasTable::parse(text).unwrap();
        assert_eq!(table.max_hops(), 3);
        let again = AliasTable::parse(&table.to_text()).unwrap();
        assert_eq!(again.to_text(), table.to_text());
        assert_eq!(again.max_hops(), 3);
    }

    #[test]
    fn a_malformed_table_fails_when_it_is_read() {
        let err = AliasTable::parse("prefix not-an-iri urn:iki:fn:").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.detail.contains("from"), "{err}");
        assert!(AliasTable::parse("rewrite a b").is_err());
        assert!(AliasTable::parse("max-hops lots").is_err());
    }

    #[test]
    fn the_overlay_resolves_the_old_name_to_the_backing_endpoint() {
        let inner = Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:iki:fn:toUpper"), builtins::to_upper()),
        );
        let table = Arc::new(AliasTable::new().prefix("urn:fn:", "urn:iki:fn:"));
        let space = Alias::new(table, inner);
        let request = Request::new(Verb::Source, iri("urn:fn:toUpper"));
        assert!(matches!(
            space.resolve(&request, &Scope::empty()),
            Resolution::Hit(_)
        ));
    }

    #[test]
    fn the_overlay_is_transparent_to_enumeration() {
        // Decision 4: the catalog sees the backing names, once each.
        let inner = Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:iki:fn:toUpper"), builtins::to_upper()),
        );
        let table = Arc::new(AliasTable::new().prefix("urn:fn:", "urn:iki:fn:"));
        let entries = Alias::new(table, inner).entries().expect("enumerable");
        let patterns: Vec<&str> = entries.iter().map(|e| e.pattern.as_str()).collect();
        assert_eq!(patterns, ["urn:iki:fn:toUpper"]);
    }

    #[test]
    fn a_refused_rewrite_resolves_to_an_endpoint_that_says_so() {
        // Not a miss: "nothing is bound there" would be a different, misleading fact.
        let inner = Arc::new(EndpointSpace::new());
        let table = Arc::new(
            AliasTable::new()
                .prefix("urn:a:", "urn:b:")
                .prefix("urn:b:", "urn:a:"),
        );
        let space = Alias::new(table, inner);
        let Resolution::Hit(hit) = space.resolve(
            &Request::new(Verb::Source, iri("urn:a:thing")),
            &Scope::empty(),
        ) else {
            panic!("a refusal must not read as a miss");
        };
        assert_eq!(hit.endpoint.describe().id, "alias-refused");
    }

    #[test]
    fn stale_scope_hints_name_grants_that_did_not_migrate() {
        let table = AliasTable::new().prefix("urn:example:fs:", "urn:example:moved:fs:");
        let held = [
            "urn:cap:fs:read:urn:example:fs:ws/x".to_string(),
            "urn:cap:kernel:inspect".to_string(),
        ];
        assert_eq!(
            table.scopes_naming_stale_namespaces(held.iter()),
            ["urn:cap:fs:read:urn:example:fs:ws/x"]
        );
    }
}
