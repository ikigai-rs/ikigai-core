//! **The cache as a graph** (ledger #907): the Turtle faces of `urn:kernel:cache`
//! and `urn:kernel:threads`, and both faces of `urn:kernel:dependents` — what a
//! golden-thread cut would recompute.
//!
//! The text readouts say what the cache holds to an operator at a terminal; these
//! say it to a reader that joins it with something else — the cache visualizer
//! (ledger #912) drawing entries grouped by the thread they hang from, or a SPARQL
//! query asking which entries a write to one resource would invalidate. Every node
//! is an IRI (no blank nodes): an entry is skolemized from its cache KEY, so the
//! same entry has the same IRI in every face and in every read until it is evicted;
//! a thread from its NAME; and an entry names the chain it was computed in by the
//! `ik:Chain` IRI `urn:kernel:topology` uses, so the cache graph joins the
//! arrangement graph.
//!
//! What is deliberately absent: the capability an entry was computed under. The
//! text readout does not show it either, and the cache key carries only a
//! fingerprint of it — the entry's IRI is derived from that key, so two callers'
//! entries for one name stay two nodes without either capability being written out.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::cache::EntryEdges;
use crate::iri::escape_iri_fragment;
use crate::topology::escape_literal;

/// Where an entry is skolemized: `{ENTRY}{16 hex digits}`, from its cache key.
const ENTRY: &str = "urn:ikigai:cache:entry:";
/// Where a golden thread is named: `{THREAD}{its name}`, IRI-escaped.
const THREAD: &str = "urn:ikigai:thread:";

const PREFIXES: &str = "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// An entry's IRI: a digest of its whole key (request, capability fingerprint,
/// chain fingerprint), so it is stable for as long as the entry is resident.
fn entry_iri(entry: &EntryEdges) -> String {
    let mut hasher = blake3::Hasher::new();
    crate::hashing::feed_str(&mut hasher, "ikigai.cache.entry.v0");
    hasher.update(entry.key.request.content_id().as_bytes());
    hasher.update(&entry.key.capability.to_le_bytes());
    hasher.update(&entry.key.scope.to_le_bytes());
    let digest = hasher.finalize();
    let short = u64::from_le_bytes(digest.as_bytes()[..8].try_into().expect("8 bytes"));
    format!("{ENTRY}{short:016x}")
}

/// A thread's IRI, from its name.
pub(crate) fn thread_iri(name: &str) -> String {
    format!("{THREAD}{}", escape_iri_fragment(name))
}

/// The `ik:Chain` IRI of a chain fingerprint, as `urn:kernel:topology` names it.
fn chain_iri(scope: u64) -> String {
    match scope {
        0 => "urn:ikigai:chain:root".to_string(),
        fingerprint => format!("urn:ikigai:chain:{fingerprint:016x}"),
    }
}

/// One entry's block.
fn write_entry(out: &mut String, entry: &EntryEdges) {
    let row = &entry.row;
    let _ = write!(
        out,
        "\n<{}> a ik:CacheEntry ;\n    ik:resolvedFrom <{}> ;\n    ik:entryState \"{}\" ;\n    \
         ik:mediaType \"{}\" ;\n    ik:sizeBytes \"{}\"^^xsd:nonNegativeInteger ;\n    ik:chain <{}>",
        entry_iri(entry),
        escape_iri_fragment(&row.target),
        row.state.word(),
        escape_literal(&row.media_type),
        row.bytes,
        chain_iri(row.scope),
    );
    let mut threads: Vec<&String> = entry.threads.iter().collect();
    threads.sort();
    for thread in threads {
        let _ = write!(out, " ;\n    ik:hangsFrom <{}>", thread_iri(thread));
    }
    out.push_str(" .\n");
}

/// One thread's block.
fn write_thread(out: &mut String, name: &str, cuts: u64) {
    let _ = write!(
        out,
        "\n<{}> a ik:GoldenThread ;\n    ik:threadName \"{}\" ;\n    ik:cutCount \"{cuts}\"^^xsd:nonNegativeInteger .\n",
        thread_iri(name),
        escape_literal(name),
    );
}

/// The Turtle face of `urn:kernel:cache`: every resident entry (stale ones marked,
/// as the text readout marks them), and every thread any of them hangs from with
/// how often it has been cut. `cuts` answers a thread's count.
pub(crate) fn cache_turtle(mut entries: Vec<EntryEdges>, cuts: impl Fn(&str) -> u64) -> String {
    entries.sort_by(|a, b| (&a.row.target, a.row.scope).cmp(&(&b.row.target, b.row.scope)));
    let mut out = String::from(PREFIXES);
    let mut threads = BTreeSet::new();
    for entry in &entries {
        write_entry(&mut out, entry);
        threads.extend(entry.threads.iter().cloned());
    }
    for thread in threads {
        let count = cuts(&thread);
        write_thread(&mut out, &thread, count);
    }
    out
}

/// The Turtle face of `urn:kernel:threads`: every tracked thread that has been cut,
/// with how often.
pub(crate) fn threads_turtle(rows: Vec<(String, u64)>) -> String {
    let rows: BTreeMap<String, u64> = rows.into_iter().collect();
    let mut out = String::from(PREFIXES);
    for (thread, cuts) in rows {
        write_thread(&mut out, &thread, cuts);
    }
    out
}

/// The text face of `urn:kernel:dependents`: the live entries hanging from
/// `thread`, one line each, with the chain each was computed in named by `chain`.
pub(crate) fn dependents_text(
    thread: &str,
    cuts: u64,
    mut entries: Vec<EntryEdges>,
    chain: impl Fn(u64) -> String,
) -> String {
    entries.sort_by(|a, b| (&a.row.target, a.row.scope).cmp(&(&b.row.target, b.row.scope)));
    let times = if cuts == 1 { "time" } else { "times" };
    let mut out = format!("dependents of {thread}  (cut {cuts} {times})\n");
    if entries.is_empty() {
        out.push_str("  (no live cached entry hangs from it: a cut would recompute nothing)\n");
        return out;
    }
    let n = entries.len();
    let _ = writeln!(
        out,
        "  {n} live cached {} — a cut makes {} stale, and the next read of each recomputes it",
        if n == 1 {
            "entry hangs from it"
        } else {
            "entries hang from it"
        },
        if n == 1 { "it" } else { "them" },
    );
    let width = entries
        .iter()
        .map(|e| e.row.target.chars().count())
        .max()
        .unwrap_or(0)
        .min(48);
    for entry in entries {
        let row = entry.row;
        let deps = if row.threads == 1 {
            "1 thread".to_string()
        } else {
            format!("{} threads", row.threads)
        };
        let _ = writeln!(
            out,
            "  {:<width$}  {:<24}  {:>9}  {deps:<10}  {}",
            row.target,
            row.media_type,
            crate::kernel::human_size(row.bytes),
            chain(row.scope)
        );
    }
    out
}

/// The Turtle face of `urn:kernel:dependents`: the thread, and the live entries
/// hanging from it — the same entry IRIs `urn:kernel:cache`'s graph uses, so the
/// two union into one picture.
pub(crate) fn dependents_turtle(thread: &str, cuts: u64, entries: Vec<EntryEdges>) -> String {
    let mut out = String::from(PREFIXES);
    write_thread(&mut out, thread, cuts);
    let mut entries = entries;
    entries.sort_by(|a, b| (&a.row.target, a.row.scope).cmp(&(&b.row.target, b.row.scope)));
    for entry in &entries {
        write_entry(&mut out, entry);
    }
    out
}
