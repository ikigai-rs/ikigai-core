//! Doors a module serves without meaning to (ledger #3796).
//!
//! A module's privacy is ADMISSION: `Mount(prefix, Level(name, inner))` admits the
//! names under `prefix`, and the module's other names are private only because the
//! guard never admits them. Nothing marks a door public or private, so a door inside
//! the level whose pattern matches an admitted name SERVES it, whatever its author
//! meant. [`check_exposure`] is the static check for that hazard: over an arrangement
//! (a [`Topology`]), it names every door inside a level whose pattern overlaps the
//! prefix the level is exposed under but which the host has not declared public
//! ([`Exposes`]). It reports; it never refuses a binding.

use crate::topology::{Door, MatchKind, SpaceKind, Topology};

/// What each module means to expose: per level (by its name, the level node's id),
/// the door patterns that are public. A pattern is compared with a door's
/// `ik:pattern` as text. A level with no entry exposes nothing on purpose, so every
/// door of it that an admitted name reaches is flagged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exposes {
    levels: Vec<(String, Vec<String>)>,
}

impl Exposes {
    /// No level declares anything public.
    pub fn new() -> Self {
        Exposes::default()
    }

    /// Declare `patterns` public for the level named `level` (builder). Declaring
    /// the same level twice adds to its list.
    pub fn level<P: Into<String>>(
        mut self,
        level: impl Into<String>,
        patterns: impl IntoIterator<Item = P>,
    ) -> Self {
        let level = level.into();
        let patterns = patterns.into_iter().map(Into::into);
        match self.levels.iter_mut().find(|(name, _)| *name == level) {
            Some((_, list)) => list.extend(patterns),
            None => self.levels.push((level, patterns.collect())),
        }
        self
    }

    fn is_public(&self, level: &str, pattern: &str) -> bool {
        self.levels
            .iter()
            .any(|(name, list)| name == level && list.iter().any(|p| p == pattern))
    }
}

/// One door [`check_exposure`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExposedDoor {
    /// The level the door is inside: the level node's id, or the skolem text
    /// `(unnamed level)` for a node built from data without one.
    pub level: String,
    /// The prefix the level is exposed under: every enclosing mount's prefix and
    /// every mount inside the level above the door, narrowed to one. Empty when no
    /// mount guards it (everything is admitted).
    pub prefix: String,
    /// The door, as the topology reports it. For an opaque space the pattern is the
    /// space's name and the kind is [`MatchKind::Custom`].
    pub door: Door,
    /// Why the door is reported, in words: what overlaps, or why it cannot be decided.
    pub reason: String,
}

/// What [`check_exposure`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExposureReport {
    /// Doors not declared public whose pattern overlaps the admitted prefix: an
    /// admitted name reaches them.
    pub flagged: Vec<ExposedDoor>,
    /// Doors (and opaque spaces) the check cannot place: a custom grammar, whose
    /// pattern describes it and does not define it; a door behind a rewrite or an
    /// alias inside the level, which may map an admitted name onto it; an opaque
    /// space. Never reported clean.
    pub undecided: Vec<ExposedDoor>,
}

impl ExposureReport {
    /// Whether nothing was flagged and nothing was left undecided.
    pub fn is_clean(&self) -> bool {
        self.flagged.is_empty() && self.undecided.is_empty()
    }
}

/// Flag every door inside a [`Level`](crate::Level) whose pattern overlaps the prefix
/// the level is exposed under but which `exposes` does not declare public.
///
/// The exposed prefix is what the mounts above the door admit: `Mount` does not strip
/// its prefix, so nested mounts narrow to the longer of two related prefixes, and two
/// unrelated ones admit nothing (no door under them is flagged). Overlap, per match
/// kind: an **exact** door overlaps when its IRI is under the prefix; a **prefix**
/// pattern when either is a prefix of the other; a **template** is placed by its
/// literal head (the text before the first `{`), and overlaps when either is a prefix
/// of the other — an over-approximation for a head the prefix extends, in the
/// direction this check can afford. A **custom** grammar is opaque: reported
/// undecided, by its pattern and endpoint. A nested level is its own module and is
/// checked against its own declaration.
///
/// ```
/// use std::sync::Arc;
/// use ikigai_core::{
///     check_exposure, EndpointSpace, Exact, Exposes, FnEndpoint, Iri, Level, Mount,
///     ReprType, Representation, Space, UriTemplate,
/// };
///
/// let ep = || FnEndpoint::new("e", |_| Ok(Representation::new(ReprType::new("text/plain"), vec![])));
/// let module: Arc<dyn Space> = Arc::new(Mount::new(
///     "urn:mod:",
///     Arc::new(Level::new(
///         Iri::parse("urn:example:level:mod").unwrap(),
///         Arc::new(
///             EndpointSpace::new()
///                 .bind(Exact::new("urn:mod:get"), ep())
///                 .bind(UriTemplate::parse("urn:mod:cache:{key}").unwrap(), ep()),
///         ),
///     )),
/// ));
/// let exposes = Exposes::new().level("urn:example:level:mod", ["urn:mod:get"]);
/// let report = check_exposure(&module.topology(), &exposes);
/// assert_eq!(report.flagged.len(), 1);
/// assert_eq!(report.flagged[0].door.pattern, "urn:mod:cache:{key}");
/// ```
pub fn check_exposure(topology: &Topology, exposes: &Exposes) -> ExposureReport {
    let mut report = ExposureReport::default();
    walk(topology, exposes, Some(String::new()), None, &mut report);
    report
}

/// Where the walk is inside a level: its name, and whether a mapper stands between
/// the level and the node (an admitted name may be rewritten before it meets a door).
#[derive(Clone)]
struct InLevel {
    name: String,
    mapped: bool,
}

/// Narrow the admitted prefix by a mount's: `None` admits nothing.
fn narrow(admitted: &Option<String>, prefix: &str) -> Option<String> {
    let a = admitted.as_ref()?;
    if prefix.starts_with(a.as_str()) {
        Some(prefix.to_string())
    } else if a.starts_with(prefix) {
        Some(a.clone())
    } else {
        None
    }
}

fn related(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn walk(
    node: &Topology,
    exposes: &Exposes,
    admitted: Option<String>,
    level: Option<InLevel>,
    report: &mut ExposureReport,
) {
    let mut admitted = admitted;
    let mut level = level;
    match &node.kind {
        SpaceKind::Mount { prefix } => admitted = narrow(&admitted, prefix),
        SpaceKind::Level { .. } => {
            level = Some(InLevel {
                name: node
                    .id
                    .as_ref()
                    .map(|i| i.as_str().to_string())
                    .unwrap_or_else(|| "(unnamed level)".into()),
                mapped: false,
            });
        }
        SpaceKind::Rewrite | SpaceKind::Alias { .. } => {
            if let Some(l) = level.as_mut() {
                l.mapped = true;
            }
        }
        SpaceKind::Opaque => {
            if let (Some(l), Some(prefix)) = (&level, &admitted) {
                let name = node
                    .id
                    .as_ref()
                    .map(|i| i.as_str().to_string())
                    .unwrap_or_else(|| "(unnamed opaque space)".into());
                report.undecided.push(ExposedDoor {
                    level: l.name.clone(),
                    prefix: prefix.clone(),
                    door: Door::new(name, MatchKind::Custom, ""),
                    reason: "an opaque space: its doors are not in the graph".into(),
                });
            }
        }
        SpaceKind::EndpointSpace { doors } => {
            if let (Some(l), Some(prefix)) = (&level, &admitted) {
                for door in doors {
                    classify(door, l, prefix, exposes, report);
                }
            }
        }
        _ => {}
    }
    for child in &node.children {
        walk(child, exposes, admitted.clone(), level.clone(), report);
    }
}

fn classify(
    door: &Door,
    level: &InLevel,
    prefix: &str,
    exposes: &Exposes,
    report: &mut ExposureReport,
) {
    if exposes.is_public(&level.name, &door.pattern) {
        return;
    }
    let found = |reason: String| ExposedDoor {
        level: level.name.clone(),
        prefix: prefix.to_string(),
        door: door.clone(),
        reason,
    };
    if level.mapped {
        report.undecided.push(found(
            "behind a rewrite or an alias inside the level: an admitted name may be mapped onto it"
                .into(),
        ));
        return;
    }
    let overlaps = match door.kind {
        MatchKind::Exact => door.pattern.starts_with(prefix),
        MatchKind::Prefix => related(&door.pattern, prefix),
        MatchKind::Template => {
            let head = door.pattern.split('{').next().unwrap_or("");
            related(head, prefix)
        }
        MatchKind::Custom => {
            report.undecided.push(found(
                "a custom grammar: its pattern describes it and does not define it".into(),
            ));
            return;
        }
    };
    if overlaps {
        report.flagged.push(found(format!(
            "not declared public, and names under `{prefix}` reach it"
        )));
    }
}
