//! **`urn:kernel:explain` — a dry run of resolution** (ledger #906): the module
//! author's "why did my endpoint not get called?" as a resource.
//!
//! The kernel already answers the operator's WHAT — `urn:kernel:topology` (the
//! arrangement), `urn:kernel:aliases` (the rewrite table and its counters),
//! `urn:kernel:cached` (is this name cached for me). What nothing answered was the
//! author's WHY for one name: which rewrites would fire, which member of the chain
//! declines and which answers, what the grammar bound, and whether this capability
//! would be refused before the endpoint is ever entered. This answers it, by taking
//! the walk resolution takes and recording it instead of invoking.
//!
//! **It never invokes an endpoint.** Everything it does is routing, which is
//! synchronous and side-effect free by contract: the kernel's rewrite table is
//! consulted through a counter-free preview (asking why a name misses must not
//! itself count as a miss at `urn:kernel:aliases`), the chain is walked by
//! [`Scope::resolve_observed`] — the same walk [`Kernel::issue`](crate::Kernel::issue)
//! takes, with an observer that records — and the answering endpoint is asked only
//! for its [`describe()`](crate::Endpoint::describe), its static contract.
//!
//! **What it does not say.** WHY a space declined: a space is a `dyn` resolver that
//! answers hit or miss, and only its own topology (when it reports one) says what is
//! inside it. The explanation reports WHAT each member did, marks a member that
//! reports no structure as opaque, and stops there honestly — a `Space::explain` is a
//! later, separate decision (Brian, 2026-10-09). Nor can it say whether the answer
//! would be cached: a description declares no cacheability, and the endpoint decides
//! per answer (ledger #38); `urn:kernel:cached` and `urn:kernel:uncached` answer that
//! after a read, live, which is why they are not joined in here — this answer is
//! cacheable, and joining live state would make it the least cacheable thing it read.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::alias::{AliasTable, Canonical, RuleKind};
use crate::capability::{is_deny_scope, Capability};
use crate::error::{Error, Result};
use crate::iri::{escape_iri_fragment, Iri};
use crate::kernel::{capability_key, parse_verb, BINDINGS_THREAD, KERNEL_NS};
use crate::repr::{ReprType, Representation};
use crate::request::Request;
use crate::seal::Seals;
use crate::space::{Consulted, Resolution, Scope, Space};
use crate::topology::{Door, SpaceKind, Topology};
use crate::verb::Verb;

/// Where a dry run's outcomes are named: `{OUTCOME}{word}` — the closed set
/// `ik:outcome` takes on an `ik:DryRun` and on each of its `ik:Consultation`s.
const OUTCOME: &str = "urn:ikigai:explain:outcome:";

/// What the kernel hands the explanation: the request asking, the asker's
/// capability and chain, and the parts of the kernel resolution reads.
pub(crate) struct Ask<'a> {
    pub(crate) request: &'a Request,
    pub(crate) capability: &'a Capability,
    pub(crate) chain: &'a Scope,
    pub(crate) root: &'a Arc<dyn Space>,
    pub(crate) seals: &'a Seals,
    pub(crate) aliases: Option<&'a AliasTable>,
}

/// One rewrite the kernel's own table would perform.
struct Hop {
    kind: RuleKind,
    from: String,
    to: String,
    before: String,
    after: String,
}

/// The kernel's rewrite of the asked name, before any space is consulted.
enum Rewrite {
    /// No rule matches.
    None,
    /// Rewritten, terminating.
    Table { hops: Vec<Hop>, canonical: Iri },
    /// Refused (a cycle, too many hops, a malformed substitution): the request
    /// would stop here.
    Refused { reason: String, trail: Vec<String> },
}

/// What one member of the chain did with the name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    /// It answered — the walk stops here.
    Answered,
    /// It answered with ⊥: the name is limited, and the caller sees Unresolved.
    Limited,
    /// It declined.
    Declined,
    /// A sealed name its frame does not own: the walk passed it by.
    Skipped,
    /// Never consulted, because a member ahead of it answered — and it WOULD have
    /// answered, with the endpoint named. The shadowed door.
    Shadowed(String),
    /// Never consulted, and it would not have answered either.
    Unreached,
}

impl Status {
    fn word(&self) -> &'static str {
        match self {
            Status::Answered => "answered",
            Status::Limited => "limited",
            Status::Declined => "declined",
            Status::Skipped => "skipped",
            Status::Shadowed(_) => "shadowed",
            Status::Unreached => "unreached",
        }
    }
}

/// One member of the chain, as the dry run saw it.
struct Member {
    label: String,
    kind: &'static str,
    opaque: bool,
    /// How many opaque spaces its reported structure encloses.
    opaque_inside: usize,
    status: Status,
    /// The member's reported doors that bind the answering endpoint's name — only
    /// on the member that answered.
    doors: Vec<Door>,
}

/// The endpoint that would answer, and what its contract says.
struct Answer {
    name: String,
    id: String,
    bindings: Vec<(String, String)>,
    answered_by: Option<Iri>,
    levels: String,
    /// A rewrite a SPACE reported on its resolution (beside the kernel's table).
    reported: Option<Iri>,
    /// Whether the endpoint's description declares the verb (an empty `verbs`
    /// declares nothing either way).
    declares_verb: Option<bool>,
    requires: Vec<String>,
    lacking: Vec<String>,
}

/// How the dry run ended.
enum Outcome {
    /// A member answers.
    Answered(Box<Answer>),
    /// A limiter answers: Unresolved, exactly as a name bound nowhere.
    Limited,
    /// Nothing answers.
    Nothing,
    /// The request would be refused before any endpoint is entered.
    Refused(String),
    /// The kernel answers the name itself, ahead of every space.
    Kernel {
        op: String,
        served: bool,
        answers_verb: bool,
        requires: Vec<String>,
        lacking: Vec<String>,
    },
}

impl Outcome {
    fn word(&self) -> &'static str {
        match self {
            Outcome::Answered(_) => "answered",
            Outcome::Limited => "limited",
            Outcome::Nothing => "unresolved",
            Outcome::Refused(_) => "refused",
            Outcome::Kernel { .. } => "kernel",
        }
    }
}

/// The whole explanation, before it is rendered in either face.
struct Explanation {
    asked: Iri,
    verb: Verb,
    /// The scopes it was answered for: `None` for root.
    held: Option<Vec<String>>,
    attenuated: bool,
    chain: String,
    chain_iri: String,
    severed: bool,
    rewrite: Rewrite,
    members: Vec<Member>,
    outcome: Outcome,
    /// The dry run's own IRI, for the Turtle face.
    node: String,
}

/// Answer `urn:kernel:explain` — the kernel's arm calls this after its inspect gate.
pub(crate) fn explain(ask: Ask<'_>) -> Result<Representation> {
    let request = ask.request;
    let target = arg(request, "target").ok_or_else(|| Error::MissingArgument("target".into()))?;
    let asked = Iri::parse(target.trim()).map_err(|e| Error::InvalidArgument {
        name: "target".to_string(),
        detail: e.to_string(),
    })?;
    let verb = match arg(request, "verb") {
        None => Verb::Source,
        Some(name) => parse_verb(name.trim())?,
    };
    let (capability, attenuated) = match arg(request, "scopes") {
        None => (ask.capability.clone(), false),
        Some(list) => (attenuate(ask.capability, list)?, true),
    };
    let turtle = match arg(request, "as").map(str::trim) {
        None | Some("text/plain") => false,
        Some("text/turtle") => true,
        Some(other) => {
            return Err(Error::InvalidArgument {
                name: "as".to_string(),
                detail: format!(
                    "`{other}` is not a face of urn:kernel:explain (text/plain, text/turtle)"
                ),
            })
        }
    };
    let explanation = walk(&ask, asked, verb, capability, attenuated);
    let (media, body) = if turtle {
        ("text/turtle", explanation.to_turtle())
    } else {
        ("text/plain", explanation.to_text())
    };
    // A face derived from the bindings, like the topology it walks: valid until the
    // binding set changes. Keyed by the asker's capability and chain by the kernel.
    Ok(Representation::new(
        ReprType::new(media).with_param("charset", "utf-8"),
        body.into_bytes(),
    )
    .cacheable()
    .depends_on(BINDINGS_THREAD))
}

/// The capability `scopes=` asks for: attenuated from the asker's, and REFUSED —
/// never silently narrowed — when it names a grant the asker does not hold. An
/// attenuation that dropped the scope quietly would answer for a capability the
/// asker did not ask about, and the answer ("would be denied") would read like a
/// fact about the endpoint rather than about the typo. An exclusion only narrows,
/// so naming one is always allowed.
fn attenuate(asker: &Capability, list: &str) -> Result<Capability> {
    let scopes: Vec<&str> = list
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(wider) = scopes
        .iter()
        .find(|scope| !is_deny_scope(scope) && !asker.allows(scope))
    {
        return Err(Error::Denied(format!(
            "urn:kernel:explain answers for a capability attenuated from the asker's, never a \
             wider one: `scopes` names `{wider}`, which the asker does not hold"
        )));
    }
    Ok(asker.attenuate(scopes))
}

/// Take the walk, recording.
fn walk(
    ask: &Ask<'_>,
    asked: Iri,
    verb: Verb,
    capability: Capability,
    attenuated: bool,
) -> Explanation {
    let chain_iri = match ask.chain.fingerprint() {
        0 => "urn:ikigai:chain:root".to_string(),
        fingerprint => format!("urn:ikigai:chain:{fingerprint:016x}"),
    };
    let node = {
        let mut hasher = blake3::Hasher::new();
        crate::hashing::feed_str(&mut hasher, "ikigai.explain.v0");
        crate::hashing::feed_str(&mut hasher, asked.as_str());
        crate::hashing::feed_u8(&mut hasher, verb.code());
        hasher.update(&ask.chain.fingerprint().to_le_bytes());
        hasher.update(&capability_key(&capability).to_le_bytes());
        let digest = hasher.finalize();
        let short = u64::from_le_bytes(digest.as_bytes()[..8].try_into().expect("8 bytes"));
        format!("urn:ikigai:explain:{short:016x}")
    };
    let mut explanation = Explanation {
        asked: asked.clone(),
        verb,
        held: capability.scopes().map(|s| s.iter().cloned().collect()),
        attenuated,
        chain: ask.chain.to_string(),
        chain_iri,
        severed: ask.chain.is_severed(),
        rewrite: Rewrite::None,
        members: Vec::new(),
        outcome: Outcome::Nothing,
        node,
    };

    // 1. The kernel's own rewrite, as `Kernel::canonicalize` would apply it — but
    //    through the counter-free preview.
    let name = match ask.aliases.map(|table| (table, table.preview(&asked))) {
        None | Some((_, Canonical::Direct)) => asked.clone(),
        Some((table, Canonical::Aliased(hop))) => {
            let mut hops = Vec::new();
            let mut current = asked.as_str().to_string();
            for &index in hop.rules() {
                let rule = &table.rules()[index];
                let after = rule.apply(&current).unwrap_or_else(|| current.clone());
                hops.push(Hop {
                    kind: rule.kind(),
                    from: rule.from().to_string(),
                    to: rule.to().to_string(),
                    before: std::mem::replace(&mut current, after.clone()),
                    after,
                });
            }
            let canonical = hop.canonical().clone();
            explanation.rewrite = Rewrite::Table {
                hops,
                canonical: canonical.clone(),
            };
            canonical
        }
        Some((_, Canonical::Refused(refusal))) => {
            explanation.rewrite = Rewrite::Refused {
                reason: refusal.reason.clone(),
                trail: refusal.trail.clone(),
            };
            explanation.outcome = Outcome::Refused(format!(
                "the kernel's rewrite table refuses `{}`: {refusal}",
                asked.as_str()
            ));
            return explanation;
        }
    };

    // 2. The kernel's own namespace is answered ahead of every space.
    if let Some(op) = name.as_str().strip_prefix(KERNEL_NS) {
        let description = crate::kernel_ops::description(op);
        let requires = crate::kernel_ops::required_scopes(op, verb);
        let lacking = requires
            .iter()
            .filter(|scope| !capability.allows(scope))
            .cloned()
            .collect();
        explanation.outcome = Outcome::Kernel {
            op: op.to_string(),
            served: description.is_some(),
            answers_verb: description.is_some_and(|d| d.verbs.contains(&verb)),
            requires,
            lacking,
        };
        return explanation;
    }

    // 3. The chain, walked the way resolution walks it.
    let probe = Request::new(verb, name);
    let mut consulted: Vec<Option<Consulted>> = Vec::new();
    let resolution = ask
        .chain
        .resolve_observed(&probe, ask.root, ask.seals, |at, outcome| {
            if consulted.len() <= at {
                consulted.resize(at + 1, None);
            }
            consulted[at] = Some(outcome);
        });
    let members = ask.chain.members(ask.root);
    let hit = match &resolution {
        Ok(Resolution::Hit(resolved)) => Some(resolved),
        _ => None,
    };
    for (at, member) in members.iter().enumerate() {
        let topology = {
            let mut node = member.space.topology();
            if node.id.is_none() {
                node.id = member.id.clone();
            }
            node
        };
        let status = match consulted.get(at).copied().flatten() {
            Some(Consulted::Hit) if hit.is_some_and(|r| r.endpoint.is_limiter()) => Status::Limited,
            Some(Consulted::Hit) => Status::Answered,
            Some(Consulted::Miss) => Status::Declined,
            Some(Consulted::Skipped) => Status::Skipped,
            // Not reached. Asking it costs one more routing call and answers the
            // question an author most often has: is my door SHADOWED?
            None => match member.space.resolve(&probe, ask.chain) {
                Resolution::Hit(would) if !would.endpoint.is_limiter() => {
                    Status::Shadowed(would.endpoint.name().to_string())
                }
                _ => Status::Unreached,
            },
        };
        let doors = match (status == Status::Answered, hit) {
            (true, Some(resolved)) => doors_naming(&topology, resolved.endpoint.name()),
            _ => Vec::new(),
        };
        explanation.members.push(Member {
            label: member.label.clone(),
            kind: kind_word(&topology.kind),
            opaque: topology.kind == SpaceKind::Opaque,
            opaque_inside: count_opaque(&topology),
            status,
            doors,
        });
    }
    // The verdict below is whatever the walk RETURNED, and each member's status is
    // what the walk reported for it — one walk, so the two cannot disagree about
    // which member answered.

    explanation.outcome = match resolution {
        Err(breach) => Outcome::Refused(breach.message()),
        Ok(Resolution::Miss) => Outcome::Nothing,
        Ok(Resolution::Hit(resolved)) if resolved.endpoint.is_limiter() => Outcome::Limited,
        Ok(Resolution::Hit(resolved)) => {
            if let Some(canonical) = resolved
                .canonical
                .as_ref()
                .filter(|c| c.as_str().starts_with(KERNEL_NS))
            {
                explanation.outcome = Outcome::Refused(format!(
                    "a space rewrote the name to `{}`, inside the kernel's own namespace — \
                     `urn:kernel:` is answered only by the kernel",
                    canonical.as_str()
                ));
                return explanation;
            }
            let description = resolved.endpoint.describe();
            let declares_verb = (!description.verbs.is_empty())
                .then(|| verb == Verb::Meta || description.verbs.contains(&verb));
            Outcome::Answered(Box::new(Answer {
                name: resolved.endpoint.name().to_string(),
                id: description.id.clone(),
                bindings: resolved
                    .bindings
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                answered_by: resolved.answered_by.clone(),
                levels: if resolved.levels().is_empty() {
                    String::new()
                } else {
                    resolved.levels().to_string()
                },
                reported: resolved.canonical.clone().filter(|c| *c != probe.target),
                declares_verb,
                requires: description.required_scopes(verb),
                lacking: description.unsatisfied_scopes(verb, &capability),
            }))
        }
    };
    explanation
}

/// The doors in `topology` (the answering member's reported structure) that bind
/// an endpoint named `name`. Names are not unique, so this can list more than the
/// door that matched — it is every door that COULD be it, which is honest; an
/// opaque member lists none.
fn doors_naming(topology: &Topology, name: &str) -> Vec<Door> {
    let mut found = Vec::new();
    let mut stack = vec![topology];
    while let Some(node) = stack.pop() {
        if let SpaceKind::EndpointSpace { doors } = &node.kind {
            found.extend(doors.iter().filter(|door| door.endpoint == name).cloned());
        }
        stack.extend(node.children.iter().rev());
    }
    found
}

/// How many opaque spaces a member's reported structure encloses (not counting
/// the member itself).
fn count_opaque(topology: &Topology) -> usize {
    topology
        .children
        .iter()
        .map(|child| usize::from(child.kind == SpaceKind::Opaque) + count_opaque(child))
        .sum()
}

/// The kind of a member, as one word for the text face.
fn kind_word(kind: &SpaceKind) -> &'static str {
    match kind {
        SpaceKind::Opaque => "opaque",
        SpaceKind::EndpointSpace { .. } => "endpoint-space",
        SpaceKind::Fallback => "fallback",
        SpaceKind::Mount { .. } => "mount",
        SpaceKind::Rewrite => "rewrite",
        SpaceKind::Alias { .. } => "alias",
        SpaceKind::Limit { .. } => "limit",
        SpaceKind::Confine => "confine",
        SpaceKind::Level { .. } => "level",
        SpaceKind::Chain { .. } => "chain",
    }
}

impl Explanation {
    fn to_text(&self) -> String {
        let mut out = String::new();
        let verb = format!("{:?}", self.verb).to_ascii_lowercase();
        let _ = writeln!(out, "explain {verb} {}", self.asked.as_str());
        let capability = match (&self.held, self.attenuated) {
            (None, _) => "root".to_string(),
            (Some(held), true) => format!("attenuated to {}", list_or_none(held)),
            (Some(held), false) => list_or_none(held),
        };
        let _ = writeln!(out, "  capability  {capability}");
        let _ = writeln!(out, "  chain       {}", self.chain);
        match &self.rewrite {
            Rewrite::None => {}
            Rewrite::Table { hops, canonical } => {
                for hop in hops {
                    let _ = writeln!(
                        out,
                        "  rewrite     {} {} -> {}   ({} => {})",
                        hop.kind.keyword(),
                        hop.from,
                        hop.to,
                        hop.before,
                        hop.after
                    );
                }
                let _ = writeln!(out, "  resolves as {}", canonical.as_str());
            }
            Rewrite::Refused { reason, trail } => {
                let _ = writeln!(
                    out,
                    "  rewrite     refused: {reason} ({})",
                    trail.join(" -> ")
                );
            }
        }
        if let Outcome::Kernel {
            op,
            served,
            answers_verb,
            requires,
            lacking,
        } = &self.outcome
        {
            let _ = writeln!(
                out,
                "  answer      the kernel itself, ahead of every space (urn:kernel:{op})"
            );
            if !served {
                let _ = writeln!(
                    out,
                    "  verdict     unresolved: the kernel serves no operation `{op}`"
                );
                return out;
            }
            if !answers_verb {
                let _ = writeln!(
                    out,
                    "  verdict     unresolved: `urn:kernel:{op}` does not answer {verb}"
                );
                return out;
            }
            write_floor(&mut out, requires, lacking);
            return out;
        }
        if !self.members.is_empty() {
            let _ = writeln!(out, "  spaces, in the order they are consulted:");
        }
        let width = self
            .members
            .iter()
            .map(|m| m.label.chars().count())
            .max()
            .unwrap_or(0)
            .min(48);
        for (at, member) in self.members.iter().enumerate() {
            let mut line = format!(
                "    {}. {:<width$}  {:<14}  {}",
                at + 1,
                member.label,
                member.kind,
                member.status.word()
            );
            match &member.status {
                Status::Shadowed(endpoint) => {
                    let _ = write!(line, " (would also answer, with `{endpoint}`: a member ahead of it answers first)");
                }
                Status::Skipped => line.push_str(" (a sealed name this level does not own)"),
                Status::Limited => line.push_str(" (a limiter: the caller sees Unresolved)"),
                _ => {}
            }
            if member.opaque {
                line.push_str(
                    " — opaque: it reports no structure, so what is inside it cannot be shown",
                );
            } else if member.opaque_inside > 0 {
                let _ = write!(
                    line,
                    " — encloses {} opaque space{}: past {}, the chain cannot be inspected",
                    member.opaque_inside,
                    if member.opaque_inside == 1 { "" } else { "s" },
                    if member.opaque_inside == 1 {
                        "it"
                    } else {
                        "them"
                    }
                );
            }
            let _ = writeln!(out, "{}", line.trim_end());
        }
        if self.severed {
            let _ = writeln!(
                out,
                "    -  root   cut off: this chain is severed (a confinement)"
            );
        }
        match &self.outcome {
            Outcome::Nothing => {
                let _ = writeln!(
                    out,
                    "  verdict     unresolved: nothing in the chain answers this name"
                );
                if matches!(self.rewrite, Rewrite::Table { .. }) {
                    let _ = writeln!(
                        out,
                        "              the rewrite moved the name onto nothing \
                         (urn:kernel:aliases counts these as unresolved)"
                    );
                }
            }
            Outcome::Limited => {
                let _ = writeln!(
                    out,
                    "  verdict     unresolved: a limiter carves this name out (the caller cannot \
                     tell it from a name bound nowhere)"
                );
            }
            Outcome::Refused(why) => {
                let _ = writeln!(
                    out,
                    "  verdict     refused before any endpoint is entered: {why}"
                );
            }
            Outcome::Answered(answer) => {
                let _ = writeln!(
                    out,
                    "  endpoint    {} (urn:ikigai:endpoint:{})",
                    answer.name,
                    escape_iri_fragment(&answer.id)
                );
                for member in &self.members {
                    for door in &member.doors {
                        let _ = writeln!(
                            out,
                            "  door        {} ({})",
                            door.pattern,
                            door.kind.keyword()
                        );
                    }
                }
                if let Some(space) = &answer.answered_by {
                    let _ = writeln!(out, "  answered by {}", space.as_str());
                }
                if !answer.levels.is_empty() {
                    let _ = writeln!(out, "  levels      {}", answer.levels);
                }
                if let Some(reported) = &answer.reported {
                    let _ = writeln!(out, "  rewritten   by a space, to {}", reported.as_str());
                }
                let bindings = if answer.bindings.is_empty() {
                    "(none)".to_string()
                } else {
                    answer
                        .bindings
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let _ = writeln!(out, "  bindings    {bindings}");
                if answer.declares_verb == Some(false) {
                    let _ = writeln!(
                        out,
                        "  verb        the endpoint does not declare {verb}; the floor below is \
                         the one an undeclared verb gets"
                    );
                }
                if self.verb == Verb::Meta {
                    let _ = writeln!(
                        out,
                        "  floor       none: the kernel answers meta from the description and never \
                         enters the endpoint"
                    );
                } else {
                    write_floor(&mut out, &answer.requires, &answer.lacking);
                }
                let cacheable = if self.verb.is_cacheable() {
                    "decided by the endpoint per answer — a description declares no cacheability; \
                     after a read, urn:kernel:cached says whether it was stored and \
                     urn:kernel:uncached why not"
                        .to_string()
                } else {
                    format!("never: a {verb} is never stored")
                };
                let _ = writeln!(out, "  cacheable   {cacheable}");
            }
            Outcome::Kernel { .. } => {}
        }
        out
    }

    fn to_turtle(&self) -> String {
        let me = &self.node;
        let mut out = String::from(
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
             @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
        );
        let _ = write!(
            out,
            "\n<{me}> a ik:DryRun ;\n    ik:asked <{}> ;\n    ik:verb \"{:?}\" ;\n    \
             ik:chain <{}> ;\n    ik:outcome <{OUTCOME}{}>",
            self.asked.as_str(),
            self.verb,
            self.chain_iri,
            self.outcome.word()
        );
        let mut below = String::new();
        if let Rewrite::Table { hops, canonical } = &self.rewrite {
            let _ = write!(out, " ;\n    ik:resolvedAs <{}>", canonical.as_str());
            for (i, hop) in hops.iter().enumerate() {
                let rule = format!("{me}:rule:{}", i + 1);
                let _ = write!(out, " ;\n    ik:rewrote <{rule}>");
                let _ = write!(
                    below,
                    "\n<{rule}> a ik:RewriteRule ;\n    ik:ruleKind \"{}\" ;\n    ik:logical \"{}\" ;\n    \
                     ik:canonical \"{}\" .\n",
                    hop.kind.keyword(),
                    literal(&hop.from),
                    literal(&hop.to)
                );
            }
        }
        for (i, member) in self.members.iter().enumerate() {
            let cell = format!("{me}:consulted:{}", i + 1);
            let _ = write!(out, " ;\n    ik:consulted <{cell}>");
            let _ = write!(
                below,
                "\n<{cell}> a ik:Consultation ;\n    ik:layer <{}:layer:{}> ;\n    \
                 ik:outcome <{OUTCOME}{}> ;\n    ik:opaque {} .\n",
                self.chain_iri,
                i + 1,
                member.status.word(),
                member.opaque
            );
        }
        let (answer, requires, lacking): (Option<String>, &[String], &[String]) = match &self
            .outcome
        {
            Outcome::Answered(answer) => {
                for (k, v) in &answer.bindings {
                    let binding = format!("{me}:binding:{}", escape_iri_fragment(k));
                    let _ = write!(out, " ;\n    ik:binding <{binding}>");
                    let _ = write!(
                        below,
                        "\n<{binding}> a ik:Binding ;\n    ik:var \"{}\" ;\n    ik:bindingValue \"{}\" .\n",
                        literal(k),
                        literal(v)
                    );
                }
                if let Some(reported) = &answer.reported {
                    let _ = write!(out, " ;\n    ik:resolvedAs <{}>", reported.as_str());
                }
                let floor: (&[String], &[String]) = if self.verb == Verb::Meta {
                    (&[], &[])
                } else {
                    (&answer.requires, &answer.lacking)
                };
                (
                    Some(format!(
                        "urn:ikigai:endpoint:{}",
                        escape_iri_fragment(&answer.id)
                    )),
                    floor.0,
                    floor.1,
                )
            }
            Outcome::Kernel {
                op,
                served: true,
                answers_verb: true,
                requires,
                lacking,
            } => (
                crate::kernel_ops::description(op)
                    .map(|d| format!("urn:ikigai:endpoint:{}", escape_iri_fragment(&d.id))),
                requires,
                lacking,
            ),
            _ => (None, &[], &[]),
        };
        if let Some(answer) = answer {
            let _ = write!(out, " ;\n    ik:answer <{answer}>");
        }
        for scope in requires {
            let _ = write!(out, " ;\n    ik:requires {}", scope_term(scope));
        }
        for scope in lacking {
            let _ = write!(out, " ;\n    ik:lacks {}", scope_term(scope));
        }
        out.push_str(" .\n");
        out.push_str(&below);
        out
    }
}

/// The capability floor's two lines: what is required, and whether this capability
/// would be refused for it — naming every scope it lacks, not just the first the
/// kernel's own refusal names.
fn write_floor(out: &mut String, requires: &[String], lacking: &[String]) {
    let _ = writeln!(out, "  requires    {}", list_or_none(requires));
    if lacking.is_empty() {
        let _ = writeln!(
            out,
            "  denied      no: the capability satisfies every declared scope"
        );
    } else {
        let _ = writeln!(
            out,
            "  denied      yes: the capability lacks {} — a real request is refused before the \
             endpoint is entered",
            lacking.join(", ")
        );
    }
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

/// A scope as a Turtle term: an IRI when it is one that can be written bracketed,
/// else a literal (a legacy descriptive label), as `ik:requires`'s comment allows.
fn scope_term(scope: &str) -> String {
    if Iri::parse(scope).is_ok() && crate::iri::is_iri_safe(scope) {
        format!("<{scope}>")
    } else {
        format!("\"{}\"", literal(scope))
    }
}

fn literal(s: &str) -> String {
    crate::topology::escape_literal(s)
}

fn arg<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    match request.args.get(name) {
        Some(crate::ArgRef::Inline(bytes)) => std::str::from_utf8(bytes).ok(),
        _ => None,
    }
}
