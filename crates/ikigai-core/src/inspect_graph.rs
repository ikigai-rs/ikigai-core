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

use crate::constraint::{TotalsReading, TotalsRow};
use crate::contract::encode_name;
use crate::iri::escape_iri_fragment;
use crate::topology::escape_literal;
use crate::uncached::REASON_KINDS;

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

/// The constraint window as a whole: how many resolutions it holds, and the first and
/// last end stamps among them, in milliseconds by the kernel's clock (ledger #4095).
/// The stamps are `None` on a clockless kernel or an empty window.
pub(crate) struct ConstraintSpan {
    pub(crate) resolutions: usize,
    pub(crate) first_ms: Option<u64>,
    pub(crate) last_ms: Option<u64>,
}

/// One target of the constraint window, aggregated over the window's samples.
///
/// A failed resolution (ledger #4095) is an uncached one: the endpoint ran and spent
/// capacity. So `failed` is a part of the uncached count and `failed_ms` a part of
/// `uncached_ms`, and a millisecond spent failing ranks exactly as one spent computing.
#[derive(Default)]
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
    /// How many of the uncached resolutions failed.
    pub(crate) failed: usize,
    /// How much of the uncached time they spent, in milliseconds (meaningful only
    /// when `timed`).
    pub(crate) failed_ms: u64,
}

impl ConstraintRow {
    pub(crate) fn uncached(&self) -> usize {
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
/// `span` is what the window holds. The span goes on the header line, after what it
/// always said, so the face keeps its shape (a header, then two lines per row) for a
/// reader that splits it.
pub(crate) fn constraint_text(span: &ConstraintSpan, rows: &[ConstraintRow]) -> String {
    let mut body = format!("constraint  (last {} resolutions)", span.resolutions);
    if let (Some(first), Some(last)) = (span.first_ms, span.last_ms) {
        let _ = write!(
            body,
            " · samples from {first}ms to {last}ms on the kernel's clock"
        );
    }
    body.push('\n');
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
        let failed = match (row.failed, row.timed) {
            (0, _) => String::new(),
            (count, true) => format!(" · {count} failed ({}ms)", row.failed_ms),
            (count, false) => format!(" · {count} failed"),
        };
        body.push_str(&format!(
            "  {}{marker}\n    {cost} · {} calls · {}% cached{failed}\n",
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
pub(crate) fn constraint_turtle(span: &ConstraintSpan, rows: &[ConstraintRow]) -> String {
    let rows: Vec<&ConstraintRow> = rows.iter().take(CONSTRAINT_TOP).collect();
    let mut out = String::from(PREFIXES);
    let _ = write!(
        out,
        "\n<{CONSTRAINT}> a ik:ConstraintWindow ;\n    \
         ik:windowResolutions \"{}\"^^xsd:nonNegativeInteger",
        span.resolutions
    );
    if let (Some(first), Some(last)) = (span.first_ms, span.last_ms) {
        let _ = write!(
            out,
            " ;\n    ik:windowFirstSampleMillis \"{first}\"^^xsd:nonNegativeInteger ;\n    \
             ik:windowLastSampleMillis \"{last}\"^^xsd:nonNegativeInteger"
        );
    }
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
             ik:cachedPercent \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:failedCallCount \"{}\"^^xsd:nonNegativeInteger",
            target_iri(&row.target),
            escape_iri_fragment(&row.target),
            rank + 1,
            row.calls,
            row.cached,
            row.uncached(),
            row.cached_pct(),
            row.failed,
        );
        if row.timed {
            let _ = write!(
                out,
                " ;\n    ik:uncachedMillis \"{}\"^^xsd:nonNegativeInteger ;\n    \
                 ik:failedMillis \"{}\"^^xsd:nonNegativeInteger",
                row.uncached_ms, row.failed_ms
            );
        }
        out.push_str(" .\n");
    }
    out
}

/// Where a totals row is skolemized: `{TOTALS_ROW}{its number}`, or `{TOTALS_ROW}other`.
/// The number is the order the row was made in, so a row keeps its IRI from one
/// reading to the next while its rank (`ik:order`) moves.
const TOTALS_ROW: &str = "urn:ikigai:constraint:totals:row:";

fn totals_row_iri(row: &TotalsRow) -> String {
    match row.number {
        Some(number) => format!("{TOTALS_ROW}{number}"),
        None => format!("{TOTALS_ROW}other"),
    }
}

/// The reasons a row's computed answers were not stored, as `(kind, count)`, in the
/// order [`REASON_KINDS`] lists them; only the kinds that occurred.
fn not_stored(row: &TotalsRow) -> impl Iterator<Item = (&'static str, u64)> + '_ {
    REASON_KINDS
        .iter()
        .zip(row.counters.not_stored.iter())
        .filter(|(_, count)| **count > 0)
        .map(|(kind, count)| (*kind, *count))
}

/// Whether a row spent capacity, so the leader is the constraint.
fn totals_consumed(row: &TotalsRow) -> bool {
    row.counters.uncached_ms > 0 || row.counters.computed > 0
}

/// The text face of `urn:kernel:constraint over=start` (ledger #4096): a header with
/// the kernel's total and start, then every row, heaviest first, two lines each (a
/// third when some answers were not stored), as the window's face lays out its rows.
pub(crate) fn totals_text(reading: &TotalsReading) -> String {
    let mut body = format!(
        "constraint  (since start: {} resolutions, {} rows)",
        reading.resolutions,
        reading.rows.len()
    );
    if let Some(started) = reading.started_ms {
        let _ = write!(body, " · started at {started}ms on the kernel's clock");
    }
    body.push('\n');
    if reading.rows.is_empty() {
        body.push_str("  (no resolutions yet)\n");
        return body;
    }
    for (rank, row) in reading.rows.iter().enumerate() {
        let marker = if rank == 0 && totals_consumed(row) {
            "   ← constraint"
        } else {
            ""
        };
        let name = match (&row.endpoint_id, &row.shape, row.folded) {
            (Some(id), Some(shape), _) => format!("{id}  {shape}"),
            (_, _, Some((count, more))) => format!(
                "(other)  {}{count} {}",
                if more { "at least " } else { "" },
                if count == 1 { "endpoint" } else { "endpoints" }
            ),
            _ => "(other)".to_string(),
        };
        let c = &row.counters;
        let cost = if reading.timed {
            format!("{}ms uncached · {} computed", c.uncached_ms, c.computed)
        } else {
            format!("{} computed", c.computed)
        };
        let failed = match (c.failed, reading.timed) {
            (0, _) => String::new(),
            (count, true) => format!(" · {count} failed ({}ms)", c.failed_ms),
            (count, false) => format!(" · {count} failed"),
        };
        let _ = writeln!(
            body,
            "  {name}{marker}\n    {cost} · {} stored · {} hits{failed}",
            c.stored, c.hits
        );
        let reasons: Vec<String> = not_stored(row)
            .map(|(kind, count)| format!("{kind} {count}"))
            .collect();
        if !reasons.is_empty() {
            let _ = writeln!(body, "    not stored: {}", reasons.join(", "));
        }
    }
    body
}

/// The Turtle face of `urn:kernel:constraint over=start` (ledger #4096), from the same
/// reading as the text face: an `ik:ConstraintTotals`, every row an
/// `ik:ConstraintTarget` in rank order, and one node per reason kind a row's answers
/// were not stored for.
pub(crate) fn totals_turtle(reading: &TotalsReading) -> String {
    let mut out = String::from(PREFIXES);
    let _ = write!(
        out,
        "\n<{CONSTRAINT}> a ik:ConstraintTotals ;\n    \
         ik:totalResolutions \"{}\"^^xsd:nonNegativeInteger",
        reading.resolutions
    );
    if let Some(started) = reading.started_ms {
        let _ = write!(
            out,
            " ;\n    ik:kernelStartedMillis \"{started}\"^^xsd:nonNegativeInteger"
        );
    }
    for row in &reading.rows {
        let _ = write!(out, " ;\n    ik:totalsRow <{}>", totals_row_iri(row));
    }
    out.push_str(" .\n");
    for (rank, row) in reading.rows.iter().enumerate() {
        let iri = totals_row_iri(row);
        let c = &row.counters;
        let id = row.endpoint_id.as_deref().unwrap_or("(other)");
        let _ = write!(
            out,
            "\n<{iri}> a ik:ConstraintTarget ;\n    ik:order \"{}\"^^xsd:integer ;\n    \
             ik:endpointId \"{}\"",
            rank + 1,
            escape_literal(id),
        );
        if let Some(shape) = &row.shape {
            let _ = write!(out, " ;\n    ik:targetShape \"{}\"", escape_literal(shape));
        }
        if let Some((count, _)) = row.folded {
            let _ = write!(
                out,
                " ;\n    ik:foldedEndpointCount \"{count}\"^^xsd:nonNegativeInteger"
            );
        }
        let _ = write!(
            out,
            " ;\n    ik:uncachedCallCount \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:storedCallCount \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:cachedCallCount \"{}\"^^xsd:nonNegativeInteger ;\n    \
             ik:failedCallCount \"{}\"^^xsd:nonNegativeInteger",
            c.computed, c.stored, c.hits, c.failed,
        );
        if reading.timed {
            let _ = write!(
                out,
                " ;\n    ik:uncachedMillis \"{}\"^^xsd:nonNegativeInteger ;\n    \
                 ik:failedMillis \"{}\"^^xsd:nonNegativeInteger",
                c.uncached_ms, c.failed_ms
            );
        }
        for (kind, _) in not_stored(row) {
            let _ = write!(
                out,
                " ;\n    ik:notStored <{iri}:reason:{}>",
                encode_name(kind)
            );
        }
        out.push_str(" .\n");
        for (kind, count) in not_stored(row) {
            let _ = write!(
                out,
                "\n<{iri}:reason:{}> ik:reasonKind \"{kind}\" ;\n    \
                 ik:count \"{count}\"^^xsd:nonNegativeInteger .\n",
                encode_name(kind),
            );
        }
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
