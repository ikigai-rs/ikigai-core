//! **The arrangement as a resource** — the paper's §8.2: the doors, corridors and
//! rules of resolution are themselves a resource among resources.
//!
//! A [`Space`](crate::Space) is a resolver behind `dyn`, and the kernel cannot see
//! inside one. So every space *reports* its own structure through
//! [`Space::topology`](crate::Space::topology): a [`Topology`] tree whose nodes say
//! what kind of space they are ([`SpaceKind`]) and, for the kinds that compose,
//! what they enclose. A space that says nothing — a remote, a hand-written
//! resolver — answers an [`Opaque`](SpaceKind::Opaque) node, which is an honest
//! answer: the graph then states that this is where structural knowledge stops.
//!
//! `urn:kernel:topology` renders the tree the request's **chain** sees — each
//! injected corridor innermost first, then the root unless the chain is severed —
//! as Turtle over the `ik:` vocabulary (`ik:Chain`, `ik:Fallback` with `ik:layers`,
//! `ik:Mount` with `ik:prefix` and `ik:space`, `ik:Limit` with `ik:family`,
//! `ik:EndpointSpace` with `ik:pattern`, `ik:Alias` with its `ik:rewrites`,
//! with `ik:prefix` and `ik:space`, `ik:Limit` with `ik:family`,
//! `ik:EndpointSpace` with `ik:pattern`, `ik:Alias` with its `ik:rewrites`,
//! `ik:Rewrite`, `ik:Confine`, `ik:Level` with `ik:space`, `ik:OpaqueSpace`).
//! with an [`id`](crate::Space::id) is named by it, and an anonymous one is
//! skolemized under `urn:ikigai:space:_:{n}` in render order — no blank nodes, so
//! the graph diffs and every list cell can be addressed. Order is explicit where it
//! is meaning: `ik:layers` is an `rdf:List` in the order resolution consults, which
//! is the order the chain's fingerprint hashes.
//!
//! What this buys is the paper's Theorem 4(b) as a **query**. Gatekeeper
//! completeness over the static tree is decidable by pushdown reachability over
//! imports and ownership; ikigai's tree contributes no pushes (the formal
//! document's §1.1), so over this graph it is a path query — "is any door of the
//! personal family reachable from the entry without a limiter over that family
//! standing ahead of it?" — that a host doctor runs before any request is made.
//! `tests/topology.rs` runs exactly that walk over the rendered graph and the
//! formal document carries it as SPARQL.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::alias::RuleKind;
use crate::iri::Iri;
use crate::space::Space;

/// What kind of space a [`Topology`] node describes, with the structure that kind
/// carries. `#[non_exhaustive]`: a kind that does not exist yet (a remote that
/// reports its transport, say) is additive.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpaceKind {
    /// A space that reports nothing about its structure — the default for any
    /// [`Space`] that does not implement [`topology`](Space::topology). Rendered
    /// `ik:OpaqueSpace`: the graph says where knowledge stops rather than
    /// implying the space is empty.
    Opaque,
    /// A leaf: the patterns of its doors, in declaration order (first match wins).
    EndpointSpace {
        /// Each door's grammar pattern — an exact IRI, or a URI template.
        patterns: Vec<String>,
    },
    /// An ordered first-hit list; the node's children are its layers in the order
    /// they are consulted.
    Fallback,
    /// A prefix-guarded import; the node's one child is the mounted space.
    Mount {
        /// The identifier prefix the mount admits.
        prefix: String,
    },
    /// A closure-driven rewrite (`τ` is a function, so the table is opaque); the
    /// node's one child is the enclosed space.
    Rewrite,
    /// A table-driven rewrite; the node's one child is the enclosed space.
    Alias {
        /// The table's rules, in table order.
        rules: Vec<TopologyRule>,
    },
    /// A limiter over a family of identifiers — a hole, not a door.
    Limit {
        /// The family: a prefix, or a grammar's pattern.
        family: String,
    },
    /// A confinement: the corridor an endpoint's sub-requests are severed into.
    /// The node's one child is the enclosed space.
    Confine,
    /// A level ([`Level`](crate::Level)): where an endpoint found inside it runs.
    /// Always named (the node's id is the level's name); the node's one child is
    /// the enclosed space. An endpoint found under it resolves its sub-requests at
    /// this node — its enclosed space, not the path that led to it — then at each
    /// enclosing level, then at the root: the one construct in the tree that
    /// PUSHES, so reachability over a tree with levels in it follows those pushes.
    /// In a chain's `ik:layers` a level is a member of the resolved scope's level
    /// stack.
    Level,
    /// The resolution chain itself — the entry node `urn:kernel:topology` answers:
    /// its children are the corridors innermost first, then the root unless the
    /// chain is severed. Not a space; the arrangement seen from one request.
    Chain {
        /// Whether the root has been cut off the chain.
        severed: bool,
    },
}

/// One rule of an [`Alias`](crate::Alias) table, as the topology reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TopologyRule {
    /// Exact or prefix.
    pub kind: RuleKind,
    /// The logical name (or prefix) the rule matches.
    pub from: String,
    /// The backing name (or prefix) it rewrites to.
    pub to: String,
}

impl TopologyRule {
    /// A rule summary.
    pub fn new(kind: RuleKind, from: impl Into<String>, to: impl Into<String>) -> Self {
        TopologyRule {
            kind,
            from: from.into(),
            to: to.into(),
        }
    }
}

/// One node of the arrangement: a space's identity (if it claims one), its kind,
/// and the spaces it encloses. Built by [`Space::topology`]; rendered by
/// [`to_turtle`](Self::to_turtle).
///
/// `#[non_exhaustive]`, so a field this crate adds later is not a flag day for a
/// consumer that builds nodes (a remote reporting its own arrangement): construct
/// with [`new`](Self::new) and the builders.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Topology {
    /// The space's own IRI, when it claims one ([`Space::id`]).
    pub id: Option<Iri>,
    /// What kind of space this is, and what that kind carries.
    pub kind: SpaceKind,
    /// The spaces this node encloses, in the order they are consulted.
    pub children: Vec<Topology>,
}

impl Topology {
    /// A node of the given kind, anonymous, with no children.
    pub fn new(kind: SpaceKind) -> Self {
        Topology {
            id: None,
            kind,
            children: Vec::new(),
        }
    }

    /// The node a space that reports nothing answers: opaque, carrying the id it
    /// claims (if any). The default [`Space::topology`].
    pub fn opaque(id: Option<Iri>) -> Self {
        Topology {
            id,
            kind: SpaceKind::Opaque,
            children: Vec::new(),
        }
    }

    /// Name the node (builder).
    pub fn with_id(mut self, id: Option<Iri>) -> Self {
        self.id = id;
        self
    }

    /// Append an enclosed space (builder).
    pub fn child(mut self, child: Topology) -> Self {
        self.children.push(child);
        self
    }

    /// The node an `Arc<dyn Space>` reports, named by its own id.
    pub fn of(space: &Arc<dyn Space>) -> Self {
        space.topology()
    }

    /// Render this node and everything under it as Turtle. When this node is a
    /// [`Chain`](SpaceKind::Chain) (what `urn:kernel:topology` answers), its IRI
    /// is the graph's entry point.
    ///
    /// Anonymous nodes are skolemized `urn:ikigai:space:_:{n}` in pre-order; a
    /// named node is its own IRI, so the same named space reached twice in a tree
    /// (a shared `Arc` under two mounts) is one node with its triples stated
    /// twice — set semantics make that harmless, and one IRI is the point.
    /// `ik:layers` is written as explicit `rdf:first`/`rdf:rest` cells under
    /// `{node}:layer:{i}`, never as `( … )`: the collection syntax parses to
    /// blank nodes, and the house rule is none.
    pub fn to_turtle(&self) -> String {
        let mut out = String::from(
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
             @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
             @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
        );
        let mut skolem = 0usize;
        self.render(&mut out, &mut skolem);
        out
    }

    /// The IRI this node is rendered under, allocating a skolem for an anonymous one.
    fn iri_for(&self, skolem: &mut usize) -> String {
        match &self.id {
            Some(id) => id.as_str().to_string(),
            None => {
                *skolem += 1;
                format!("urn:ikigai:space:_:{skolem}")
            }
        }
    }

    /// Render this node's triples, then its children's, into `out`. Returns the
    /// IRI the node was rendered under, so a parent can point at it. The node
    /// takes its skolem before its children take theirs (pre-order), and its block
    /// is written ahead of theirs, so a reader meets a node before what it encloses.
    fn render(&self, out: &mut String, skolem: &mut usize) -> String {
        let me = self.iri_for(skolem);
        let mut below = String::new();
        let children: Vec<String> = self
            .children
            .iter()
            .map(|child| child.render(&mut below, skolem))
            .collect();
        match &self.kind {
            SpaceKind::Opaque => {
                let _ = write!(out, "\n<{me}> a ik:OpaqueSpace .\n");
            }
            SpaceKind::EndpointSpace { patterns } => {
                let _ = write!(out, "\n<{me}> a ik:EndpointSpace");
                for pattern in patterns {
                    let _ = write!(out, " ;\n    ik:pattern \"{}\"", escape(pattern));
                }
                out.push_str(" .\n");
            }
            SpaceKind::Fallback => {
                let _ = write!(out, "\n<{me}> a ik:Fallback");
                write_layers(out, &me, &children);
            }
            SpaceKind::Mount { prefix } => {
                let _ = write!(
                    out,
                    "\n<{me}> a ik:Mount ;\n    ik:prefix \"{}\"",
                    escape(prefix)
                );
                write_space(out, &children);
            }
            SpaceKind::Rewrite => {
                let _ = write!(out, "\n<{me}> a ik:Rewrite");
                write_space(out, &children);
            }
            SpaceKind::Alias { rules } => {
                let _ = write!(out, "\n<{me}> a ik:Alias");
                for i in 1..=rules.len() {
                    let _ = write!(out, " ;\n    ik:rewrites <{me}:rule:{i}>");
                }
                write_space(out, &children);
                for (i, rule) in rules.iter().enumerate() {
                    let _ = write!(
                        out,
                        "\n<{me}:rule:{}> a ik:RewriteRule ;\n    ik:ruleKind \"{}\" ;\n    \
                         ik:logical \"{}\" ;\n    ik:canonical \"{}\" .\n",
                        i + 1,
                        rule.kind.keyword(),
                        escape(&rule.from),
                        escape(&rule.to)
                    );
                }
            }
            SpaceKind::Limit { family } => {
                let _ = write!(
                    out,
                    "\n<{me}> a ik:Limit ;\n    ik:family \"{}\" .\n",
                    escape(family)
                );
            }
            SpaceKind::Confine => {
                let _ = write!(out, "\n<{me}> a ik:Confine");
                write_space(out, &children);
            }
            SpaceKind::Level => {
                let _ = write!(out, "\n<{me}> a ik:Level");
                write_space(out, &children);
            }
            SpaceKind::Chain { severed } => {
                let _ = write!(out, "\n<{me}> a ik:Chain ;\n    ik:severed {severed}");
                write_layers(out, &me, &children);
            }
        }
        out.push_str(&below);
        me
    }
}

/// `ik:space <child>` for a one-child node, or close the block for a node whose
/// child reported nothing.
fn write_space(block: &mut String, children: &[String]) {
    for child in children {
        let _ = write!(block, " ;\n    ik:space <{child}>");
    }
    block.push_str(" .\n");
}

/// `ik:layers` as explicit list cells: `<me:layer:1> rdf:first <a> ; rdf:rest
/// <me:layer:2>` … ending in `rdf:nil`. An empty list is `rdf:nil` directly.
fn write_layers(block: &mut String, me: &str, children: &[String]) {
    if children.is_empty() {
        block.push_str(" ;\n    ik:layers rdf:nil .\n");
        return;
    }
    let _ = write!(block, " ;\n    ik:layers <{me}:layer:1> .\n");
    for (i, child) in children.iter().enumerate() {
        let next = if i + 1 == children.len() {
            "rdf:nil".to_string()
        } else {
            format!("<{me}:layer:{}>", i + 2)
        };
        let _ = write!(
            block,
            "<{me}:layer:{}> rdf:first <{child}> ;\n    rdf:rest {next} .\n",
            i + 1
        );
    }
}

/// Escape a literal for a Turtle `"…"` position.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
