//! **The scheduler and the constraint as graphs** (ledger #1800): the Turtle faces of
//! `urn:kernel:scheduler` and `urn:kernel:constraint`, beside their text readouts, so an
//! inspector can render them as graph views the way it renders `urn:kernel:cache`,
//! `threads` and `topology`.
//!
//! Each face is computed from the SAME rows as its text readout, so the two faces cannot
//! disagree about a value. Every node is an IRI (no blank nodes):
//!
//! - the scheduler is one node, `<urn:kernel:scheduler> a ik:Scheduler`. The rows a
//!   [`SchedulerReporter`](crate::SchedulerReporter) gives that the kernel knows (the
//!   default's and the cli scheduler's: `backend`, `threads`, `active`, `spawned`,
//!   `completed`) become typed properties. Any other row, or a known one whose value
//!   does not fit its type, or a known label given twice, is kept as an `ik:Reading`
//!   (label and value, verbatim, with its position in the reporter's rows), never
//!   dropped: a host adds rows without a core change, and its rows must reach the graph.
//! - the constraint window is `<urn:kernel:constraint> a ik:ConstraintWindow`, and each
//!   ranked target is a node skolemized from the target's name as
//!   `urn:ikigai:constraint:target:{name}`, in the rank order the text shows.

use std::fmt::Write as _;

use crate::contract::encode_name;
use crate::iri::escape_iri_fragment;
use crate::topology::escape_literal;

const PREFIXES: &str = "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// The subject of the scheduler face: the resource itself.
const SCHEDULER: &str = "urn:kernel:scheduler";
/// Where a scheduler row the kernel does not type is skolemized: `{READING}{position}`.
const READING: &str = "urn:ikigai:scheduler:reading:";
/// The subject of the constraint face: the resource itself.
const CONSTRAINT: &str = "urn:kernel:constraint";
/// Where a ranked target is skolemized: `{TARGET}{its name, encoded as a NAME}`.
const TARGET: &str = "urn:ikigai:constraint:target:";

/// How many targets both faces of `urn:kernel:constraint` list.
pub(crate) const CONSTRAINT_TOP: usize = 5;

/// The rows `urn:kernel:scheduler` reports with no reporter injected: the
/// runtime-free single-threaded default.
pub(crate) fn default_scheduler_rows() -> Vec<(String, String)> {
    vec![
        ("backend".to_string(), "single".to_string()),
        ("threads".to_string(), "1".to_string()),
    ]
}

/// The text face of `urn:kernel:scheduler`.
pub(crate) fn scheduler_text(rows: &[(String, String)]) -> String {
    let mut body = String::from("scheduler\n");
    for (label, value) in rows {
        body.push_str(&format!("  {label:<10} {value}\n"));
    }
    body
}

/// The typed property a known scheduler row label maps to, and whether its value is a
/// count (`xsd:nonNegativeInteger`) rather than a string.
fn scheduler_property(label: &str) -> Option<(&'static str, bool)> {
    match label {
        "backend" => Some(("ik:schedulerBackend", false)),
        "threads" => Some(("ik:workerThreads", true)),
        "active" => Some(("ik:activeTasks", true)),
        "spawned" => Some(("ik:spawnedTasks", true)),
        "completed" => Some(("ik:completedTasks", true)),
        _ => None,
    }
}

/// The Turtle face of `urn:kernel:scheduler`, from the same rows as the text face.
pub(crate) fn scheduler_turtle(rows: &[(String, String)]) -> String {
    let mut out = String::from(PREFIXES);
    let _ = write!(out, "\n<{SCHEDULER}> a ik:Scheduler");
    let mut typed: Vec<&str> = Vec::new();
    let mut readings: Vec<(usize, &str, &str)> = Vec::new();
    for (position, (label, value)) in rows.iter().enumerate() {
        let property = scheduler_property(label).filter(|(property, count)| {
            !typed.contains(property) && (!count || value.parse::<u64>().is_ok())
        });
        match property {
            Some((property, true)) => {
                typed.push(property);
                let _ = write!(
                    out,
                    " ;\n    {property} \"{value}\"^^xsd:nonNegativeInteger"
                );
            }
            Some((property, false)) => {
                typed.push(property);
                let _ = write!(out, " ;\n    {property} \"{}\"", escape_literal(value));
            }
            None => readings.push((position, label, value)),
        }
    }
    for (position, _, _) in &readings {
        let _ = write!(out, " ;\n    ik:reading <{READING}{position}>");
    }
    out.push_str(" .\n");
    for (position, label, value) in readings {
        let _ = write!(
            out,
            "\n<{READING}{position}> a ik:Reading ;\n    ik:readingLabel \"{}\" ;\n    \
             ik:readingValue \"{}\" ;\n    ik:order \"{position}\"^^xsd:integer .\n",
            escape_literal(label),
            escape_literal(value),
        );
    }
    out
}

/// One target of the constraint window, aggregated over the window's samples.
pub(crate) struct ConstraintRow {
    /// The target's name, as requested.
    pub(crate) target: String,
    /// Uncached compute time, in milliseconds (meaningful only when `timed`).
    pub(crate) uncached_ms: u64,
    /// Resolutions of this target in the window.
    pub(crate) calls: usize,
    /// How many of them the cache served.
    pub(crate) cached: usize,
    /// Whether any sample carried a time (the kernel has a clock).
    pub(crate) timed: bool,
}

impl ConstraintRow {
    fn uncached(&self) -> usize {
        self.calls - self.cached
    }

    fn cached_pct(&self) -> usize {
        (self.cached * 100).checked_div(self.calls).unwrap_or(0)
    }

    /// Whether this row consumed capacity, so a leader is the constraint.
    fn consumed(&self) -> bool {
        self.uncached_ms > 0 || self.uncached() > 0
    }
}

/// The text face of `urn:kernel:constraint`. `rows` are ranked, heaviest first;
/// `total` is how many resolutions the window holds.
pub(crate) fn constraint_text(total: usize, rows: &[ConstraintRow]) -> String {
    let mut body = format!("constraint  (last {total} resolutions)\n");
    if rows.is_empty() {
        body.push_str("  (no resolutions yet)\n");
        return body;
    }
    for (rank, row) in rows.iter().take(CONSTRAINT_TOP).enumerate() {
        // The leader is the constraint only if it actually consumed capacity.
        let marker = if rank == 0 && row.consumed() {
            "   ← constraint"
        } else {
            ""
        };
        let cost = if row.timed {
            format!("{}ms uncached", row.uncached_ms)
        } else {
            format!("{} uncached", row.uncached())
        };
        body.push_str(&format!(
            "  {}{marker}\n    {cost} · {} calls · {}% cached\n",
            row.target,
            row.calls,
            row.cached_pct()
        ));
    }
    body
}

/// A ranked target's node IRI, from its name. The name is a NAME here (it is carried
/// verbatim by `ik:constraintResource`), so it is encoded as one, as a thread is.
fn target_iri(target: &str) -> String {
    format!("{TARGET}{}", encode_name(target))
}

/// The Turtle face of `urn:kernel:constraint`, from the same ranked rows as the text
/// face: the window, the targets it lists in rank order (`ik:order`, from 1), and the
/// leader as `ik:currentConstraint` exactly when the text marks it.
pub(crate) fn constraint_turtle(total: usize, rows: &[ConstraintRow]) -> String {
    let rows: Vec<&ConstraintRow> = rows.iter().take(CONSTRAINT_TOP).collect();
    let mut out = String::from(PREFIXES);
    let _ = write!(
        out,
        "\n<{CONSTRAINT}> a ik:ConstraintWindow ;\n    \
         ik:windowResolutions \"{total}\"^^xsd:nonNegativeInteger"
    );
    if let Some(leader) = rows.first().filter(|row| row.consumed()) {
        let _ = write!(
            out,
            " ;\n    ik:currentConstraint <{}>",
            target_iri(&leader.target)
        );
    }
    for row in &rows {
        let _ = write!(out, " ;\n    ik:rankedTarget <{}>", target_iri(&row.target));
    }
    out.push_str(" .\n");
    for (rank, row) in rows.iter().enumerate() {
        let _ = write!(
            out,
            "\n<{}> a ik:ConstraintTarget ;\n    ik:constraintResource <{}> ;\n    \
             ik:order \"{}\"^^xsd:integer ;\n    \
             ik:callCount \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:cachedCallCount \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:uncachedCallCount \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:cachedPercent \"{}\"^^xsd:nonNegativeInteger",
            target_iri(&row.target),
            escape_iri_fragment(&row.target),
            rank + 1,
            row.calls,
            row.cached,
            row.uncached(),
            row.cached_pct(),
        );
        if row.timed {
            let _ = write!(
                out,
                " ;\n    ik:uncachedMillis \"{}\"^^xsd:nonNegativeInteger",
                row.uncached_ms
            );
        }
        out.push_str(" .\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(l, v)| (l.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_default_text_is_the_readout_it_always_was() {
        assert_eq!(
            scheduler_text(&default_scheduler_rows()),
            "scheduler\n  backend    single\n  threads    1\n"
        );
    }

    #[test]
    fn an_unknown_row_a_bad_count_and_a_repeat_become_readings() {
        let turtle = scheduler_turtle(&rows(&[
            ("backend", "pool"),
            ("threads", "many"),
            ("queue", "3"),
            ("backend", "other"),
        ]));
        assert!(turtle.contains("ik:schedulerBackend \"pool\""), "{turtle}");
        assert!(!turtle.contains("ik:workerThreads"), "{turtle}");
        for position in [1, 2, 3] {
            assert!(
                turtle.contains(&format!("ik:reading <{READING}{position}>")),
                "{turtle}"
            );
        }
        assert!(turtle.contains("ik:readingValue \"many\""), "{turtle}");
        assert!(turtle.contains("ik:readingLabel \"queue\""), "{turtle}");
        assert!(turtle.contains("ik:readingValue \"other\""), "{turtle}");
    }
}
