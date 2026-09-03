//! Selection — match a need against the self-descriptions in a [`Space`], the same way at
//! two scales:
//!
//! - [`select_transreptor`] — find a chain of transreptors converting a representation from
//!   one **media type** to another, using the `from`/`to` each transreptor declares
//!   ([`EndpointKind::Transreptor`](crate::EndpointKind)). This is what metadata rendering,
//!   content-negotiation, and sniff-and-dispatch build on: "give me a way from media type A
//!   to B." v1 finds a **direct single hop**, else a **two-hop pivot via the canonical RDF
//!   type (`text/turtle`)** — the hub our transreptors share.
//! - [`select_action`] — find endpoints whose required inputs are satisfiable by the **RDF
//!   classes** present in a context: "given these typed entities, what can I do with them?"
//!   (the seed of layer action-inference). Matches on [`ArgSpec::class`](crate::ArgSpec).
//!
//! Both are RDF-free Rust walks over the same `entries → Meta → describe` path the catalog
//! uses (no SPARQL, no oxigraph in core). General N-hop path-finding, a cached selection
//! index, and capability-scoped filtering are later refinements; today each enumerates per
//! call.
//!
//! Only **auto-invocable** transreptors are selected — ones drivable with just a piped
//! `content` and a target `as` (see [`is_auto_invocable`]). A *parameterized* transreptor
//! like `urn:xslt:transform` (which requires a `stylesheet`) is still a transreptor for
//! discovery, but can't be invoked automatically, so it's excluded here.

use std::sync::Arc;

use crate::describe::{Description, InputSource};
use crate::grammar::{Bindings, UriTemplate};
use crate::iri::Iri;
use crate::request::Request;
use crate::space::{Resolution, Scope, Space, SpaceEntry};
use crate::verb::Verb;

/// The canonical RDF media type the transreptor graph hubs on — the pivot for two-hop
/// conversions when no direct transreptor exists.
pub const CANONICAL: &str = "text/turtle";

/// One step of a transreption plan: invoke the transreptor at `endpoint` with the input
/// piped as `content` and `as` set to `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransreptionStep {
    /// The transreptor endpoint's IRI.
    pub endpoint: String,
    /// The media type to request from it (its `as`).
    pub to: String,
}

/// A transreptor's declared conversions, paired with its IRI.
struct Candidate {
    iri: String,
    from: Vec<String>,
    to: Vec<String>,
}

impl Candidate {
    fn handles(&self, from: &str, to: &str) -> bool {
        self.from.iter().any(|f| f == from) && self.to.iter().any(|t| t == to)
    }
}

/// Whether a transreptor can be invoked automatically — driven with only a piped `content`
/// and a target `as`. True iff every *required* input is `content` or `as`. A transreptor
/// with another required input (e.g. `urn:xslt:transform`'s `stylesheet`) is parameterized
/// and must be invoked explicitly, so it is not auto-selected.
pub fn is_auto_invocable(description: &Description) -> bool {
    description
        .inputs
        .iter()
        .filter(|i| i.required)
        .all(|i| i.name == "content" || i.name == "as")
}

/// Find a chain of auto-invocable transreptors in `root` converting `from` → `to`: a direct
/// single hop if one exists, else a two-hop pivot via [`CANONICAL`]. `None` if no chain is
/// available (or if `from == to`, which needs no transreption).
pub fn select_transreptor(root: &dyn Space, from: &str, to: &str) -> Option<Vec<TransreptionStep>> {
    if from == to {
        return None;
    }
    let candidates = collect(root);

    // Direct: a single transreptor that reads `from` and produces `to`.
    if let Some(c) = candidates.iter().find(|c| c.handles(from, to)) {
        return Some(vec![TransreptionStep {
            endpoint: c.iri.clone(),
            to: to.to_string(),
        }]);
    }

    // Pivot: `from → text/turtle` then `text/turtle → to` (two distinct hops).
    if from != CANONICAL && to != CANONICAL {
        let first = candidates.iter().find(|c| c.handles(from, CANONICAL))?;
        let second = candidates.iter().find(|c| c.handles(CANONICAL, to))?;
        return Some(vec![
            TransreptionStep {
                endpoint: first.iri.clone(),
                to: CANONICAL.to_string(),
            },
            TransreptionStep {
                endpoint: second.iri.clone(),
                to: to.to_string(),
            },
        ]);
    }

    None
}

/// Enumerate `root`'s auto-invocable transreptors (the same `entries → Meta → describe`
/// walk the catalog uses). Template-bound entries are deliberately excluded: a
/// transreption step invokes its endpoint with only `content` + `as`, so a pattern
/// needing binding arguments to form its IRI can never be auto-invoked.
fn collect(root: &dyn Space) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for entry in root.entries().unwrap_or_default() {
        let Ok(iri) = Iri::parse(&entry.pattern) else {
            continue;
        };
        if let Resolution::Hit(resolved) =
            root.resolve(&Request::new(Verb::Meta, iri), &Scope::empty())
        {
            let description = resolved.endpoint.describe();
            if let Some(t) = description.transreption() {
                if is_auto_invocable(&description) {
                    candidates.push(Candidate {
                        iri: entry.pattern.clone(),
                        from: t.from.clone(),
                        to: t.to.clone(),
                    });
                }
            }
        }
    }
    candidates
}

/// Convenience: select over an `Arc<dyn Space>` root (as a kernel holds).
pub fn select_transreptor_in(
    root: &Arc<dyn Space>,
    from: &str,
    to: &str,
) -> Option<Vec<TransreptionStep>> {
    select_transreptor(root.as_ref(), from, to)
}

/// The placeholder each `{var}` takes when a template pattern is probe-expanded into a
/// concrete IRI for `Meta` resolution (see [`describe_entry`]). The value is arbitrary:
/// `describe()` does not depend on bindings, and [`describe_entry`]'s identity guard
/// catches the case where the probe IRI resolves elsewhere.
const PROBE: &str = "probe";

/// A space entry's self-description, with how its pattern names the endpoint: `None`
/// for an exact, directly resolvable IRI; `Some(vars)` for a URI-template pattern
/// whose variables must be supplied (as `Binding`-source arguments) to form one.
pub(crate) struct EntryDescription {
    /// The bound endpoint's `describe()`.
    pub description: Description,
    /// The template's variable names, when the pattern is a template grammar.
    pub template_vars: Option<Vec<String>>,
}

/// Describe one space entry — the shared step of every `entries → Meta → describe`
/// walk (catalog, selection, validate's id lookup). An exact pattern IS the IRI to
/// `Meta`-resolve. A template pattern (`urn:file:{path}`) is not an IRI at all, so it is
/// **probe-expanded**: each `{var}` takes a placeholder, and the concrete probe IRI is
/// resolved the normal way — reaching the same endpoint the template binds, since
/// `describe()` does not depend on bindings. Resolution is first-match-wins, so a probe
/// IRI *could* land on a different binding; such a hit is discarded (better invisible
/// than misdescribed). `None` for patterns that are neither parseable IRIs nor parseable
/// templates, and for misses.
///
/// The guard takes **either witness**: the resolved endpoint's `name()`, or the id of the
/// description it hands back — the very artifact about to be attached to this pattern.
/// One witness is not enough, because over a **mounted** space they are not equally
/// authoritative. A mount resolves nothing locally: it always hits with a forwarder whose
/// `name()` is the client's *guess*, replayed from the remote's pattern STRINGS
/// first-match-wins and blind to the remote's real grammar semantics (ikigai-browse's PR
/// row rejects an `n` spanning a `:`, which no pattern string can express). The forwarder's
/// `describe()` is a Meta round-trip to the remote — its own answer for that very IRI —
/// so the description is right where the label is wrong. On a name-only guard the two
/// PR-grain rows (`…:pr:{n}:explain`, `…:pr:{n}:review`) were swallowed by the shorter
/// `…:pr:{n}` sibling's guess and vanished from every mounted manifold and MCP tool list,
/// while resolving correctly through the REPL.
pub(crate) fn describe_entry(root: &dyn Space, entry: &SpaceEntry) -> Option<EntryDescription> {
    if let Ok(iri) = Iri::parse(&entry.pattern) {
        let Resolution::Hit(resolved) =
            root.resolve(&Request::new(Verb::Meta, iri), &Scope::empty())
        else {
            return None;
        };
        return Some(EntryDescription {
            description: resolved.endpoint.describe(),
            template_vars: None,
        });
    }
    let template = UriTemplate::parse(&entry.pattern).ok()?;
    let vars: Vec<String> = template.variables().map(str::to_string).collect();
    if vars.is_empty() {
        return None; // no variables ⇒ just a malformed IRI, not a template
    }
    let mut bindings = Bindings::new();
    for var in &vars {
        bindings.insert(var.clone(), PROBE);
    }
    let probe = Iri::parse(template.expand(&bindings)?).ok()?;
    let Resolution::Hit(resolved) = root.resolve(&Request::new(Verb::Meta, probe), &Scope::empty())
    else {
        return None;
    };
    let description = resolved.endpoint.describe();
    if resolved.endpoint.name() != entry.endpoint && description.id != entry.endpoint {
        return None;
    }
    Some(EntryDescription {
        description,
        template_vars: Some(vars),
    })
}

/// Whether a template action is drivable from its declared contract: every template
/// variable must be a declared `Binding`-source input ([`ArgSpec::binding`]
/// (crate::ArgSpec::binding)), so a caller — engine, MCP projection, agent — knows to
/// substitute it into the pattern to form the concrete IRI. A template variable that is
/// undeclared (or declared as a by-value argument) leaves the IRI unconstructible from
/// the contract alone, so the action stays out of the manifold — the same principle that
/// keeps untyped required inputs out of typed selection.
fn template_drivable(action: &crate::describe::ActionSpec, vars: &[String]) -> bool {
    vars.iter().all(|var| {
        action
            .inputs
            .iter()
            .any(|i| i.name == *var && i.source == InputSource::Binding)
    })
}

/// One selected action — an (endpoint, verb) pair whose contract the query satisfied: its
/// required capability scopes are allowed, its verb/output fit the asked-for shape, and its
/// required typed inputs are satisfiable by the present RDF classes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionMatch {
    /// The bound pattern — what you invoke. For an exact grammar this is the endpoint's
    /// resolvable IRI; for a template grammar it is the URI-template pattern itself
    /// (`urn:file:{path}`), whose `Binding`-source arguments substitute in to form the
    /// concrete IRI.
    pub endpoint: String,
    /// The endpoint's description id (the catalog subject is `urn:ikigai:endpoint:{id}`).
    pub id: String,
    /// The matched verb.
    pub verb: Verb,
    /// The action node's catalog IRI (`urn:ikigai:endpoint:{id}:action:{verb}`) — joins
    /// this match to the full contract in the catalog graph.
    pub action: String,
    /// The capability scopes the action requires (all satisfied by the query's capability).
    pub requires: Vec<String>,
    /// How many *optional typed* inputs the present classes could not fill — the v1
    /// ranking: fewer = a better-fitted match.
    pub missing_optional: usize,
}

/// A selection query — every axis optional, so the degenerate query lists the caller's
/// whole capability-scoped action manifold ("what can I do at all?").
#[derive(Clone, Copy, Debug, Default)]
pub struct ActionQuery<'a> {
    /// RDF classes present in the context (entities you hold). Empty = no type filter.
    pub present: &'a [&'a str],
    /// Only actions with this verb.
    pub verb: Option<Verb>,
    /// Only actions that can produce this media type.
    pub want: Option<&'a str>,
    /// The caller's capability: actions whose `requires` it does not allow are NOT
    /// offered. This is the same [`Capability::allows`](crate::Capability::allows) check
    /// enforcement uses at invoke time — selection is a pre-flight of enforcement, never a
    /// substitute — so an attenuated agent's manifold simply lacks what it may not do.
    /// `None` = no capability filter (equivalent to root).
    pub capability: Option<&'a crate::Capability>,
}

/// Find endpoints in `root` whose required inputs are *fully typed and satisfiable* by
/// `present` — the RDF classes available in a context. An endpoint matches iff it has at
/// least one required input and **every** required input declares an
/// [`ArgSpec::class`](crate::ArgSpec) that appears in `present`. This is
/// [`select_transreptor`]'s sibling at the RDF-class level — "given these typed entities,
/// what can I do with them?" — the seed of layer action-inference. Optional inputs are
/// ignored; required inputs without a declared class make an endpoint un-inferable (it can't
/// be driven from the present types alone), so it's excluded. Capability-scoping — offering
/// only what the caller may invoke — composes on top and is **not** applied here.
pub fn select_action(root: &dyn Space, present: &[&str]) -> Vec<ActionMatch> {
    let query = ActionQuery {
        present,
        ..Default::default()
    };
    // The historical per-endpoint view: dedup the per-action matches by endpoint.
    let mut matches = select_actions(root, &query);
    let mut seen = std::collections::BTreeSet::new();
    matches.retain(|m| seen.insert(m.endpoint.clone()));
    matches
}

/// The action-level selection funnel: walk every bound endpoint's normalized per-verb
/// contracts ([`Description::action_specs`]) and keep the actions the query satisfies —
/// capability first (the caller never sees what it may not invoke), then verb, then the
/// wanted output type, then type-satisfiability of required inputs. Ordered
/// best-fitted-first ([`ActionMatch::missing_optional`], then endpoint/verb for
/// determinism).
pub fn select_actions(root: &dyn Space, query: &ActionQuery) -> Vec<ActionMatch> {
    let mut matches = Vec::new();
    for entry in root.entries().unwrap_or_default() {
        let Some(described) = describe_entry(root, &entry) else {
            continue;
        };
        let description = described.description;
        for action in description.action_specs() {
            if let Some(vars) = &described.template_vars {
                if !template_drivable(&action, vars) {
                    continue;
                }
            }
            if let Some(verb) = query.verb {
                if action.verb != verb {
                    continue;
                }
            }
            if let Some(want) = query.want {
                if !action.outputs.iter().any(|o| o == want) {
                    continue;
                }
            }
            if let Some(capability) = query.capability {
                if !action
                    .requires
                    .iter()
                    .all(|scope| cap_satisfies(capability, scope))
                {
                    continue;
                }
            }
            if !query.present.is_empty() && !spec_satisfiable(&action, query.present) {
                continue;
            }
            let missing_optional = action
                .inputs
                .iter()
                .filter(|i| !i.required)
                .filter(|i| {
                    i.class
                        .as_deref()
                        .is_some_and(|c| !query.present.contains(&c))
                })
                .count();
            let verb_name = format!("{:?}", action.verb).to_lowercase();
            matches.push(ActionMatch {
                endpoint: entry.pattern.clone(),
                id: description.id.clone(),
                verb: action.verb,
                action: format!("urn:ikigai:endpoint:{}:action:{verb_name}", description.id),
                requires: action.requires.clone(),
                missing_optional,
            });
        }
    }
    matches.sort_by(|a, b| {
        (a.missing_optional, &a.endpoint, a.verb as u8).cmp(&(
            b.missing_optional,
            &b.endpoint,
            b.verb as u8,
        ))
    });
    matches
}

/// Whether `capability` satisfies one required scope — [`Capability::allows`] for a plain
/// scope, or, for a `…:*` wildcard, "holds ANY grant under this prefix". The wildcard is
/// how parameterized capability grammars (`urn:cap:net:<host-rule>`,
/// `urn:cap:fs:<action>:<path>`) annotate their actions: no single static IRI names what
/// they require, because authorization depends on the argument (which selection doesn't
/// have yet). Offering-level semantics only — enforcement at invoke time still checks the
/// exact target against the ACL.
pub(crate) fn cap_satisfies(capability: &crate::Capability, scope: &str) -> bool {
    match scope.strip_suffix('*') {
        Some(prefix) => match capability.scopes() {
            None => true, // root
            Some(held) => held.iter().any(|s| s.starts_with(prefix)),
        },
        None => capability.allows(scope),
    }
}

/// Whether every required input of `action` declares a class present in `present`, with
/// at least one required input (see [`select_action`]).
fn spec_satisfiable(action: &crate::describe::ActionSpec, present: &[&str]) -> bool {
    let mut has_required = false;
    for input in action.inputs.iter().filter(|i| i.required) {
        has_required = true;
        match &input.class {
            Some(class) if present.contains(&class.as_str()) => {}
            _ => return false,
        }
    }
    has_required
}

/// Convenience: select actions over an `Arc<dyn Space>` root (as a kernel holds).
pub fn select_action_in(root: &Arc<dyn Space>, present: &[&str]) -> Vec<ActionMatch> {
    select_action(root.as_ref(), present)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::describe::Description;
    use crate::endpoint::{Endpoint, FnEndpoint};
    use crate::grammar::Exact;
    use crate::repr::{ReprType, Representation};
    use crate::space::EndpointSpace;

    /// A stub transreptor: declares from/to (auto-invocable: content + as), does nothing.
    fn transreptor(id: &'static str, from: &[&str], to: &[&str]) -> FnEndpoint {
        FnEndpoint::new(id, |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new(id)
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("content"))
                .input(crate::describe::ArgSpec::new("as"))
                .transreptor(from.iter().copied(), to.iter().copied()),
        )
    }

    fn space() -> EndpointSpace {
        EndpointSpace::new()
            // turtle <-> rdf/xml, n-triples, html (an rdf-transrept-like hub)
            .bind(
                Exact::new("urn:rdf:transrept"),
                transreptor(
                    "rdf",
                    &[
                        "text/turtle",
                        "application/rdf+xml",
                        "application/n-triples",
                    ],
                    &[
                        "text/turtle",
                        "application/rdf+xml",
                        "application/n-triples",
                        "text/html",
                    ],
                ),
            )
            // a plain (non-transreptor) endpoint — must be ignored
            .bind(
                Exact::new("urn:fn:toUpper"),
                FnEndpoint::new("toUpper", |_inv| {
                    Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
                }),
            )
    }

    #[test]
    fn finds_a_direct_hop() {
        let plan = select_transreptor(&space(), "application/rdf+xml", "text/turtle").unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].endpoint, "urn:rdf:transrept");
        assert_eq!(plan[0].to, "text/turtle");
    }

    #[test]
    fn pivots_via_turtle_when_no_direct_hop() {
        // rdf/xml → html: no single transreptor declares that pair directly here? It does
        // (rdf handles rdf+xml→html). Use a case needing the pivot: add a turtle→csv-only
        // transreptor and ask rdf+xml → text/csv.
        let space = space().bind(
            Exact::new("urn:demo:csv"),
            transreptor("csv", &["text/turtle"], &["text/csv"]),
        );
        let plan = select_transreptor(&space, "application/rdf+xml", "text/csv").unwrap();
        assert_eq!(plan.len(), 2, "{plan:?}");
        assert_eq!(plan[0].to, "text/turtle"); // pivot
        assert_eq!(plan[1].endpoint, "urn:demo:csv");
        assert_eq!(plan[1].to, "text/csv");
    }

    #[test]
    fn none_when_unreachable_or_identity() {
        assert!(select_transreptor(&space(), "text/turtle", "text/turtle").is_none());
        assert!(select_transreptor(&space(), "application/pdf", "image/png").is_none());
    }

    #[test]
    fn parameterized_transreptors_are_not_auto_invocable() {
        // An xslt-like transreptor with a required `stylesheet` is excluded from selection.
        let xslt = FnEndpoint::new("xslt", |_inv| {
            Ok(Representation::new(ReprType::new("text/html"), Vec::new()))
        })
        .with_description(
            Description::new("xslt")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("content"))
                .input(crate::describe::ArgSpec::new("stylesheet"))
                .input(crate::describe::ArgSpec::new("as"))
                .transreptor(["application/xml"], ["text/html"]),
        );
        assert!(!is_auto_invocable(&xslt.describe()));
        let space = EndpointSpace::new().bind(Exact::new("urn:xslt:transform"), xslt);
        assert!(select_transreptor(&space, "application/xml", "text/html").is_none());
    }

    // --- select_action ---

    const PERSON: &str = "https://schema.org/Person";
    const PLACE: &str = "https://schema.org/Place";
    const DATE: &str = "https://schema.org/Date";

    /// An endpoint with the given required, typed inputs (each input named `inN`, classed).
    fn typed_action(id: &'static str, classes: &[&str]) -> FnEndpoint {
        let mut d = Description::new(id).verb(Verb::Source);
        for (n, class) in classes.iter().enumerate() {
            d = d.input(crate::describe::ArgSpec::new(format!("in{n}")).class(*class));
        }
        FnEndpoint::new(id, |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(d)
    }

    fn action_space() -> EndpointSpace {
        EndpointSpace::new()
            // schedule(Person, Place, Date) — the "invite to dinner" action.
            .bind(
                Exact::new("urn:demo:schedule"),
                typed_action("schedule", &[PERSON, PLACE, DATE]),
            )
            // greet(Person) — satisfiable from just a Person.
            .bind(
                Exact::new("urn:demo:greet"),
                typed_action("greet", &[PERSON]),
            )
            // a plain untyped endpoint (content/as) — never an inferred action.
            .bind(
                Exact::new("urn:rdf:transrept"),
                transreptor("rdf", &["text/turtle"], &["text/html"]),
            )
    }

    fn endpoints(matches: &[ActionMatch]) -> Vec<&str> {
        let mut v: Vec<&str> = matches.iter().map(|m| m.endpoint.as_str()).collect();
        v.sort();
        v
    }

    #[test]
    fn action_matches_when_all_required_typed_inputs_are_present() {
        // Canvas with a Person, a Place, and Date(s) → both schedule and greet are offerable.
        let m = select_action(&action_space(), &[PERSON, PLACE, DATE]);
        assert_eq!(endpoints(&m), vec!["urn:demo:greet", "urn:demo:schedule"]);
    }

    #[test]
    fn action_excluded_when_a_required_type_is_missing() {
        // Only a Person present → greet matches, schedule (needs Place + Date) does not.
        let m = select_action(&action_space(), &[PERSON]);
        assert_eq!(endpoints(&m), vec!["urn:demo:greet"]);
    }

    // --- template grammars in the manifold ---

    use crate::describe::ActionSpec;
    use crate::grammar::UriTemplate;

    /// A file-like endpoint bound by template: one Source action, capability-gated,
    /// with its template variable declared as a Binding-source input.
    fn template_file_endpoint() -> FnEndpoint {
        FnEndpoint::new("file", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("file").action(
                ActionSpec::new(Verb::Source)
                    .requires("urn:cap:fs:read:*")
                    .input(
                        crate::describe::ArgSpec::new("path")
                            .summary("captured from the IRI")
                            .binding(),
                    ),
            ),
        )
    }

    #[test]
    fn a_template_action_with_declared_binding_args_joins_the_manifold() {
        let space = EndpointSpace::new().bind(
            UriTemplate::parse("urn:file:{path}").unwrap(),
            template_file_endpoint(),
        );

        // Under a capability holding a grant beneath the wildcard, the action is offered
        // — and the match carries the PATTERN string round-trip, not a probe IRI.
        let reader = crate::Capability::scoped(["urn:cap:fs:read:/notes"]);
        let query = ActionQuery {
            capability: Some(&reader),
            ..Default::default()
        };
        let m = select_actions(&space, &query);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].endpoint, "urn:file:{path}");
        assert_eq!(m[0].id, "file");
        assert_eq!(m[0].verb, Verb::Source);
        assert_eq!(m[0].action, "urn:ikigai:endpoint:file:action:source");

        // Under a capability with no fs grant, the manifold simply lacks it.
        let denied = crate::Capability::scoped(["urn:cap:unrelated"]);
        let query = ActionQuery {
            capability: Some(&denied),
            ..Default::default()
        };
        assert!(select_actions(&space, &query).is_empty());
    }

    #[test]
    fn a_template_whose_variables_lack_binding_argspecs_stays_out() {
        // `path` declared as a by-value ARGUMENT, not a Binding: the contract gives a
        // caller no way to construct the concrete IRI, so the action is not offered —
        // the same principle that keeps untyped required inputs out of typed selection.
        let undeclared = FnEndpoint::new("file", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("file")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("path")),
        );
        let space =
            EndpointSpace::new().bind(UriTemplate::parse("urn:file:{path}").unwrap(), undeclared);
        assert!(select_actions(&space, &ActionQuery::default()).is_empty());
    }

    #[test]
    fn a_shadowed_probe_is_discarded_not_misattributed() {
        // The probe IRI for `urn:t:{v}:x` is `urn:t:probe:x` — bound here, FIRST, to a
        // different endpoint. Resolution hands back the shadow; the name guard rejects
        // it rather than attaching the shadow's description to the template pattern.
        let shadow = FnEndpoint::new("shadow", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(Description::new("shadow").verb(Verb::Source).input(
            crate::describe::ArgSpec::new("v").binding(), // even "drivable" on paper
        ));
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:t:probe:x"), shadow)
            .bind(
                UriTemplate::parse("urn:t:{v}:x").unwrap(),
                template_file_endpoint(),
            );
        let matches = select_actions(&space, &ActionQuery::default());
        assert!(
            !matches.iter().any(|m| m.endpoint == "urn:t:{v}:x"),
            "shadowed template must stay invisible, not lie: {matches:?}"
        );
        // The exact binding itself is still offered normally.
        assert!(matches.iter().any(|m| m.endpoint == "urn:t:probe:x"));
    }

    // --- template rows behind a MOUNT ---

    /// The two rows a mounted browse family binds for one root, in the remote's own
    /// order: the shorter one ends in a variable and is a prefix of the longer.
    const MOUNTED_ROWS: [(&str, &str); 2] = [
        ("urn:t:pr:{n}", "pr-page"),
        ("urn:t:pr:{n}:explain", "pr-explain"),
    ];

    /// The REMOTE's honest resolution of a concrete IRI: its `pr:{n}` grammar rejects an
    /// `n` spanning a `:` (exactly what ikigai-browse's PR row does), so the probe IRI
    /// `urn:t:pr:probe:explain` genuinely reaches the `:explain` endpoint over there.
    fn remote_endpoint_of(target: &str) -> Option<&'static str> {
        let rest = target.strip_prefix("urn:t:pr:")?;
        match rest.strip_suffix(":explain") {
            Some(n) if !n.is_empty() && !n.contains(':') => Some("pr-explain"),
            _ => (!rest.is_empty() && !rest.contains(':')).then_some("pr-page"),
        }
    }

    /// The CLIENT's guess at the remote endpoint's name: the remote's pattern strings
    /// replayed first-match-wins, which is all a mount has locally (`RemoteNames` in
    /// ikigai-resolve). It cannot see the `:`-rejection above, so the shorter row's
    /// trailing variable swallows the longer row's probe IRI.
    fn guessed_endpoint_of(target: &Iri) -> Option<&'static str> {
        use crate::grammar::Grammar;
        MOUNTED_ROWS.iter().find_map(|(pattern, endpoint)| {
            UriTemplate::parse(*pattern)
                .ok()?
                .match_iri(target)
                .map(|_| *endpoint)
        })
    }

    /// A stand-in for a mounted remote space (ikigai-cli's `MountedRemote`): it resolves
    /// nothing itself — every request is forwarded — so it always hands back a forwarder
    /// whose `name()` is the local guess and whose `describe()` is the remote's own
    /// answer for that very IRI.
    struct MountFace;

    struct Forwarder {
        guessed: &'static str,
        described: Description,
    }

    #[async_trait::async_trait]
    impl crate::Endpoint for Forwarder {
        async fn invoke(&self, _inv: &crate::Invocation<'_>) -> crate::Result<Representation> {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        }

        fn name(&self) -> &str {
            self.guessed
        }

        fn describe(&self) -> Description {
            self.described.clone()
        }
    }

    fn mounted_description(id: &str) -> Description {
        Description::new(id).action(
            ActionSpec::new(Verb::Source).input(crate::describe::ArgSpec::new("n").binding()),
        )
    }

    impl Space for MountFace {
        fn resolve(&self, request: &Request, _scope: &Scope) -> Resolution {
            // A mount never misses: routing is the prefix's job.
            Resolution::Hit(crate::space::Resolved::new(
                Arc::new(Forwarder {
                    guessed: guessed_endpoint_of(&request.target).unwrap_or("remote"),
                    described: mounted_description(
                        remote_endpoint_of(request.target.as_str()).unwrap_or("remote"),
                    ),
                }),
                Bindings::new(),
            ))
        }

        fn entries(&self) -> Option<Vec<SpaceEntry>> {
            Some(
                MOUNTED_ROWS
                    .iter()
                    .map(|(pattern, endpoint)| SpaceEntry::new(*pattern, *endpoint))
                    .collect(),
            )
        }
    }

    #[test]
    fn a_mounted_template_row_survives_a_sibling_that_shadows_only_the_local_guess() {
        let matches = select_actions(&MountFace, &ActionQuery::default());
        assert_eq!(
            endpoints(&matches),
            vec!["urn:t:pr:{n}", "urn:t:pr:{n}:explain"],
            "both mounted rows join the manifold: the longer row's probe IRI is what the \
             REMOTE resolves, and its description says so, even though the local \
             first-match-wins guess labels it with the shorter sibling's endpoint"
        );
        // The right description is attached — not the shadowing sibling's.
        let explain = matches
            .iter()
            .find(|m| m.endpoint == "urn:t:pr:{n}:explain")
            .expect("the explain row is offered");
        assert_eq!(explain.id, "pr-explain");
        assert_eq!(
            explain.action,
            "urn:ikigai:endpoint:pr-explain:action:source"
        );
    }

    #[test]
    fn a_row_whose_description_also_disowns_it_stays_invisible() {
        // Neither witness names the entry's endpoint — the guard still discards it, so a
        // genuinely misresolved probe never attaches a foreign contract to a pattern.
        struct Disowned;
        impl Space for Disowned {
            fn resolve(&self, _request: &Request, _scope: &Scope) -> Resolution {
                Resolution::Hit(crate::space::Resolved::new(
                    Arc::new(Forwarder {
                        guessed: "someone-else",
                        described: mounted_description("someone-else"),
                    }),
                    Bindings::new(),
                ))
            }

            fn entries(&self) -> Option<Vec<SpaceEntry>> {
                Some(vec![SpaceEntry::new("urn:t:pr:{n}:explain", "pr-explain")])
            }
        }
        assert!(select_actions(&Disowned, &ActionQuery::default()).is_empty());
    }

    #[test]
    fn untyped_and_no_required_endpoints_are_never_inferred_actions() {
        // The transreptor's required inputs (content/as) carry no class → not an action,
        // even when every present type is offered.
        let m = select_action(&action_space(), &[PERSON, PLACE, DATE]);
        assert!(!endpoints(&m).contains(&"urn:rdf:transrept"));

        // An endpoint with no required inputs is not an inferred action either.
        let space = EndpointSpace::new().bind(
            Exact::new("urn:demo:ping"),
            FnEndpoint::new("ping", |_inv| {
                Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
            }),
        );
        assert!(select_action(&space, &[PERSON]).is_empty());
    }
}
