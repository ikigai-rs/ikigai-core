//! **The constraint's totals since the kernel started** (ledger #4096): what
//! `urn:kernel:constraint over=start` answers, beside the rolling window `over=window`
//! has always answered. `docs/design/constraint-history.md`, sections 3b and 4, is the
//! design; this is its build step 3.
//!
//! The window forgets: 512 resolutions, then the oldest goes. A host that wants to know
//! where the constraint was last week reads these totals every few minutes and
//! subtracts two readings, which loses nothing the kernel saw, because every counter
//! here only ever grows.
//!
//! **One row per endpoint, never per name.** A row is found by the endpoint that
//! computed the answer, not by the target: a template endpoint makes a new name for
//! every argument, and a table keyed by name would grow with the arguments and record
//! them. The row is named once, when it is made, by the endpoint's `describe().id` and
//! the target's *shape*: the IRI with each captured template value replaced by
//! `{name}` ([`target_shape`]). No argument and no template value is ever kept.
//!
//! **How an endpoint finds its row.** The floor memo, which already holds every bound
//! endpoint by its `Arc` address with a `Weak` reserving that address, keeps the row
//! beside the floor, so a stably bound endpoint finds its row with the floor and pays
//! nothing more. An endpoint the memo does not hold (a volatile contract, an entry the
//! memo swept) is found by its address here ([`ConstraintTotals::row_for`]), with the
//! same `Weak` reservation. An allocation never seen before is matched by its name: an
//! overlay that wraps the endpoint in a fresh `Arc` per resolution (a dynamic space's
//! thread, a throttle governor) is one row, not one per request, and so is an endpoint
//! that was rebound. Two endpoints a reader could not tell apart (one id at one shape)
//! share a row.
//!
//! **Bounded without eviction.** At most [`TOTALS_ROWS`] rows. Past that, a new
//! endpoint is counted in one `(other)` row, which states how many distinct names it
//! holds. Nothing is evicted, so every row is monotone and a difference of two readings
//! is exact.
//!
//! **A cache hit credits the row that computed the entry.** A hit invokes no endpoint,
//! so each cache entry carries the row of the endpoint that computed it (one `u32`).
//!
//! **Under the window's lock.** The totals live beside the window in the one `Mutex`
//! `record_resolution` already takes; there is no second lock on the resolution path.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};

use crate::endpoint::Endpoint;
use crate::grammar::{Bindings, UriTemplate};
use crate::repr::Time;
use crate::uncached::REASON_KINDS;

/// How many rows the totals hold before new endpoints fold into `(other)`. A host's
/// count of bound endpoints is fixed and small; mounted peers and dynamic spaces are
/// the only way near it (design note, section 8: set above the largest host's count).
pub(crate) const TOTALS_ROWS: usize = 256;

/// The row of a cache entry stored with no computing endpoint (the public
/// [`ReprCache::store`](crate::ReprCache::store)), and of an endpoint the memo was
/// asked about without a resolution (the cache-status probe). Recording under it counts
/// the resolution and credits no row.
pub(crate) const NO_ROW: u32 = u32::MAX;

/// The `(other)` row's index.
pub(crate) const OTHER: u32 = TOTALS_ROWS as u32;

/// How many endpoint addresses the index holds before it forgets the dead ones (and,
/// if they are all alive, all of them: it is an index, and a name finds a row again).
const ADDRESS_INDEX_BOUND: usize = 1024;

/// How many distinct names the `(other)` row counts exactly; past it, it says "at
/// least". A ceiling on memory, not a tuning knob.
const FOLDED_NAMES_BOUND: usize = 4096;

/// How many captured values [`target_shape`] tries every order of before it settles
/// for the first placement it finds.
const SHAPE_PERMUTATION_LIMIT: usize = 5;

/// What one recorded resolution adds to its row: which row, and for a computed answer
/// whether it was stored and, if not, which [`REASON_KINDS`] say why (a mask).
#[derive(Clone, Copy)]
pub(crate) struct Tally {
    pub(crate) row: u32,
    pub(crate) stored: bool,
    pub(crate) reasons: u16,
}

impl Tally {
    /// A cache hit, or a failure: only the row.
    pub(crate) fn row(row: u32) -> Self {
        Tally {
            row,
            stored: false,
            reasons: 0,
        }
    }
}

/// How a recorded resolution ended, for the totals.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    Computed,
    Cached,
    Failed,
}

/// One row's counters since the kernel started. Every field only grows. The six that
/// every computation writes come first (`repr(C)` keeps them there); the reason counts
/// follow, written only when an answer was not stored.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct Counters {
    /// Resolutions an endpoint ran for: computed answers and failures alike.
    pub(crate) computed: u64,
    /// Of `computed`, the answers the cache stored.
    pub(crate) stored: u64,
    /// Of `computed`, the ones that failed.
    pub(crate) failed: u64,
    /// Resolutions the cache served from an entry this row computed.
    pub(crate) hits: u64,
    /// Milliseconds spent on `computed`, by the kernel's clock, inclusive of
    /// sub-requests (failures included).
    pub(crate) uncached_ms: u64,
    /// Of `uncached_ms`, the milliseconds spent on failures.
    pub(crate) failed_ms: u64,
    /// Per [`REASON_KINDS`] entry, how many computed answers were not stored for it.
    /// An answer with two reasons counts under both.
    pub(crate) not_stored: [u64; REASON_KINDS.len()],
}

impl Counters {
    fn add(&mut self, ended: Ended, elapsed_ms: Option<u64>, tally: Tally) {
        match ended {
            Ended::Cached => self.hits += 1,
            Ended::Computed | Ended::Failed => {
                self.computed += 1;
                let ms = elapsed_ms.unwrap_or(0);
                self.uncached_ms += ms;
                if ended == Ended::Failed {
                    self.failed += 1;
                    self.failed_ms += ms;
                } else if tally.stored {
                    self.stored += 1;
                }
                let mut reasons = tally.reasons;
                while reasons != 0 {
                    let kind = reasons.trailing_zeros() as usize;
                    if let Some(count) = self.not_stored.get_mut(kind) {
                        *count += 1;
                    }
                    reasons &= reasons - 1;
                }
            }
        }
    }
}

/// One named row. Its counters start a cache line, so the six a record writes are one
/// line: under the lock, each line written is a transfer between cores when threads
/// contend for it. Rows live on the heap (in the `Vec`, and `(other)` boxed), so the
/// alignment widens no field of the kernel itself.
#[repr(C, align(64))]
struct Row {
    counters: Counters,
    endpoint_id: String,
    shape: String,
}

/// The totals since the kernel started: the rows, the `(other)` row, and the indexes
/// that find a row.
pub(crate) struct ConstraintTotals {
    /// When the kernel started, by its clock: a reading with a different start than
    /// the previous one is a restart, and its counters began again from zero. `None`
    /// on a kernel with no clock.
    pub(crate) started: Option<Time>,
    /// Every resolution recorded, whatever its row.
    resolutions: u64,
    rows: Vec<Row>,
    other: Box<Row>,
    /// Hashes of the distinct names folded into `(other)`, up to [`FOLDED_NAMES_BOUND`].
    folded: HashSet<u64>,
    /// Whether more names folded than `folded` counts.
    folded_more: bool,
    /// A row by its name, `(endpoint id, shape)`.
    by_name: HashMap<(String, String), u32>,
    /// A row by its endpoint's address; the `Weak` reserves the address (as the floor
    /// memo's does), so a hit here is the same allocation.
    by_address: HashMap<usize, (Weak<dyn Endpoint>, u32)>,
}

/// One row of a reading.
#[derive(Clone, Debug)]
pub(crate) struct TotalsRow {
    /// The row's number, from 0 in the order rows were made; `None` for `(other)`.
    pub(crate) number: Option<usize>,
    /// The endpoint's `describe().id`; `None` for `(other)`.
    pub(crate) endpoint_id: Option<String>,
    /// The target's shape; `None` for `(other)`.
    pub(crate) shape: Option<String>,
    /// For `(other)`, how many distinct names it holds, and whether that is a floor.
    pub(crate) folded: Option<(usize, bool)>,
    pub(crate) counters: Counters,
}

/// One reading of the totals: what `over=start` renders.
#[derive(Clone, Debug)]
pub(crate) struct TotalsReading {
    pub(crate) started_ms: Option<u64>,
    pub(crate) resolutions: u64,
    /// Every row, heaviest first (uncached milliseconds, then computed count, then
    /// row number), `(other)` among them when anything folded into it.
    pub(crate) rows: Vec<TotalsRow>,
    /// Whether the kernel has a clock (milliseconds are meaningful).
    pub(crate) timed: bool,
}

fn address(endpoint: &Arc<dyn Endpoint>) -> usize {
    Arc::as_ptr(endpoint) as *const () as usize
}

fn name_hash(endpoint_id: &str, shape: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    endpoint_id.hash(&mut hasher);
    shape.hash(&mut hasher);
    hasher.finish()
}

impl ConstraintTotals {
    pub(crate) fn new() -> Self {
        ConstraintTotals {
            started: None,
            resolutions: 0,
            rows: Vec::new(),
            other: Box::new(Row {
                counters: Counters::default(),
                endpoint_id: String::new(),
                shape: String::new(),
            }),
            folded: HashSet::new(),
            folded_more: false,
            by_name: HashMap::new(),
            by_address: HashMap::new(),
        }
    }

    /// The row `endpoint` is indexed under by its address, if any.
    pub(crate) fn row_at(&self, endpoint: &Arc<dyn Endpoint>) -> Option<u32> {
        let (weak, row) = self.by_address.get(&address(endpoint))?;
        // The reservation makes this a tautology, as in the floor memo.
        debug_assert_eq!(Weak::as_ptr(weak) as *const () as usize, address(endpoint));
        Some(*row)
    }

    /// The row for `endpoint`, named `(endpoint_id, shape)` if it is new: found by its
    /// address, else by its name, else made (or folded into `(other)` past
    /// [`TOTALS_ROWS`]). Either way the address is indexed.
    pub(crate) fn row_for(
        &mut self,
        endpoint: &Arc<dyn Endpoint>,
        endpoint_id: &str,
        shape: String,
    ) -> u32 {
        if let Some(row) = self.row_at(endpoint) {
            return row;
        }
        let row = match self.by_name.get(&(endpoint_id.to_string(), shape.clone())) {
            Some(row) => *row,
            None if self.rows.len() < TOTALS_ROWS => {
                let row = self.rows.len() as u32;
                self.rows.push(Row {
                    endpoint_id: endpoint_id.to_string(),
                    shape: shape.clone(),
                    counters: Counters::default(),
                });
                self.by_name.insert((endpoint_id.to_string(), shape), row);
                row
            }
            None => {
                let hash = name_hash(endpoint_id, &shape);
                if self.folded.len() < FOLDED_NAMES_BOUND {
                    self.folded.insert(hash);
                } else if !self.folded.contains(&hash) {
                    self.folded_more = true;
                }
                OTHER
            }
        };
        if self.by_address.len() >= ADDRESS_INDEX_BOUND {
            self.by_address
                .retain(|_, (weak, _)| weak.strong_count() > 0);
            if self.by_address.len() >= ADDRESS_INDEX_BOUND {
                self.by_address.clear();
            }
        }
        self.by_address
            .insert(address(endpoint), (Arc::downgrade(endpoint), row));
        row
    }

    /// Count one resolution, and credit `tally.row` with it.
    pub(crate) fn record(&mut self, ended: Ended, elapsed_ms: Option<u64>, tally: Tally) {
        self.resolutions += 1;
        let counters = match tally.row {
            OTHER => &mut self.other.counters,
            row => match self.rows.get_mut(row as usize) {
                Some(row) => &mut row.counters,
                None => return,
            },
        };
        counters.add(ended, elapsed_ms, tally);
    }

    /// A reading: every row, ranked as the window ranks its targets.
    pub(crate) fn reading(&self, timed: bool) -> TotalsReading {
        let mut rows: Vec<TotalsRow> = self
            .rows
            .iter()
            .enumerate()
            .map(|(number, row)| TotalsRow {
                number: Some(number),
                endpoint_id: Some(row.endpoint_id.clone()),
                shape: Some(row.shape.clone()),
                folded: None,
                counters: row.counters.clone(),
            })
            .collect();
        if !self.folded.is_empty() || self.other.counters != Counters::default() {
            rows.push(TotalsRow {
                number: None,
                endpoint_id: None,
                shape: None,
                folded: Some((self.folded.len(), self.folded_more)),
                counters: self.other.counters.clone(),
            });
        }
        rows.sort_by(|a, b| {
            b.counters
                .uncached_ms
                .cmp(&a.counters.uncached_ms)
                .then(b.counters.computed.cmp(&a.counters.computed))
                .then(
                    a.number
                        .unwrap_or(usize::MAX)
                        .cmp(&b.number.unwrap_or(usize::MAX)),
                )
        });
        TotalsReading {
            started_ms: self.started.map(|t| t.as_millis()),
            resolutions: self.resolutions,
            rows,
            timed,
        }
    }
}

/// The target's **shape**: `target` with each captured template value in `bindings`
/// replaced by `{name}`, so `urn:file:/home/alice/notes.md` matched by `urn:file:{path}`
/// is `urn:file:{path}`. A name bound exactly captures nothing and is its own shape.
///
/// The resolution carries the values, not the template, so the template is
/// reconstructed: each value is placed at an occurrence in the target, right to left
/// (a final `{var}` captures the remainder, so its value is a suffix), and a placement
/// is accepted first when the shape, parsed as a template, matches the target back to
/// exactly these bindings. A value that does not occur verbatim (a grammar of the
/// host's own that decodes what it captures) makes the whole shape `(unshaped)`:
/// recording less is the safe failure, and the endpoint id still names the row.
pub(crate) fn target_shape(target: &str, bindings: &Bindings) -> String {
    let values: Vec<(&str, &str)> = bindings.iter().filter(|(_, v)| !v.is_empty()).collect();
    if values.is_empty() {
        return target.to_string();
    }
    if values.iter().any(|(_, value)| !target.contains(value)) {
        return "(unshaped)".to_string();
    }
    // The first guess: the values in descending order of where they last occur.
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(target.rfind(values[i].1)));
    let mut fallback: Option<String> = None;
    let mut try_order = |order: &[usize]| -> Option<String> {
        let shape = place(target, &values, order)?;
        if verifies(&shape, target, &values) {
            return Some(shape);
        }
        fallback.get_or_insert(shape);
        None
    };
    if let Some(shape) = try_order(&order) {
        return shape;
    }
    if values.len() <= SHAPE_PERMUTATION_LIMIT {
        let mut found = None;
        permutations(
            &mut (0..values.len()).collect::<Vec<_>>(),
            0,
            &mut |order| {
                if found.is_none() {
                    found = try_order(order);
                }
            },
        );
        if let Some(shape) = found {
            return shape;
        }
    }
    fallback.unwrap_or_else(|| "(unshaped)".to_string())
}

/// Place each value of `order`, right to left, at its rightmost occurrence that ends
/// at or before where the previous one began; `None` if one does not fit.
fn place(target: &str, values: &[(&str, &str)], order: &[usize]) -> Option<String> {
    let mut cursor = target.len();
    let mut spans: Vec<(usize, usize, &str)> = Vec::with_capacity(order.len());
    for &i in order {
        let (name, value) = values[i];
        let start = target[..cursor].rfind(value)?;
        spans.push((start, start + value.len(), name));
        cursor = start;
    }
    spans.sort_by_key(|span| span.0);
    let mut shape = String::with_capacity(target.len());
    let mut at = 0;
    for (start, end, name) in spans {
        shape.push_str(&target[at..start]);
        shape.push('{');
        shape.push_str(name);
        shape.push('}');
        at = end;
    }
    shape.push_str(&target[at..]);
    Some(shape)
}

/// Whether `shape`, as a template, matches `target` back to exactly `values`.
fn verifies(shape: &str, target: &str, values: &[(&str, &str)]) -> bool {
    let Ok(template) = UriTemplate::parse(shape) else {
        return false;
    };
    let Some(matched) = template.match_with_empty(target, &[]) else {
        return false;
    };
    matched.iter().count() == values.len()
        && values
            .iter()
            .all(|(name, value)| matched.get(name) == Some(*value))
}

/// Heap's algorithm, calling `visit` with each order of `items`.
fn permutations(items: &mut Vec<usize>, k: usize, visit: &mut dyn FnMut(&[usize])) {
    if k == items.len() {
        visit(items);
        return;
    }
    for i in k..items.len() {
        items.swap(k, i);
        permutations(items, k + 1, visit);
        items.swap(k, i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(pairs: &[(&str, &str)]) -> Bindings {
        let mut b = Bindings::new();
        for (k, v) in pairs {
            b.insert(*k, *v);
        }
        b
    }

    #[test]
    fn an_exact_name_is_its_own_shape() {
        assert_eq!(
            target_shape("urn:books:graph", &Bindings::new()),
            "urn:books:graph"
        );
    }

    #[test]
    fn a_captured_value_becomes_its_name() {
        assert_eq!(
            target_shape(
                "urn:file:/home/alice/notes.md",
                &bindings(&[("path", "/home/alice/notes.md")])
            ),
            "urn:file:{path}"
        );
        assert_eq!(
            target_shape(
                "urn:user:alice:inbox:42",
                &bindings(&[("who", "alice"), ("n", "42")])
            ),
            "urn:user:{who}:inbox:{n}"
        );
    }

    #[test]
    fn a_value_that_also_appears_in_the_literal_text_is_still_placed() {
        // `urn:file:{path}` with path `file`: the final variable is the suffix.
        assert_eq!(
            target_shape("urn:file:file", &bindings(&[("path", "file")])),
            "urn:file:{path}"
        );
        // Two values, one inside the other's text.
        assert_eq!(
            target_shape("urn:a:ab:b", &bindings(&[("x", "ab"), ("y", "b")])),
            "urn:a:{x}:{y}"
        );
    }

    #[test]
    fn a_value_not_in_the_target_unshapes_it() {
        assert_eq!(
            target_shape("urn:x:a%20b", &bindings(&[("v", "a b")])),
            "(unshaped)"
        );
    }

    #[test]
    fn counters_count_reasons_by_kind() {
        let mut c = Counters::default();
        c.add(
            Ended::Computed,
            Some(5),
            Tally {
                row: 0,
                stored: false,
                reasons: 0b100 | 1 << 11,
            },
        );
        c.add(
            Ended::Computed,
            Some(2),
            Tally {
                row: 0,
                stored: true,
                reasons: 0,
            },
        );
        c.add(Ended::Failed, Some(7), Tally::row(0));
        c.add(Ended::Cached, Some(1), Tally::row(0));
        assert_eq!(
            (
                c.computed,
                c.stored,
                c.failed,
                c.hits,
                c.uncached_ms,
                c.failed_ms
            ),
            (3, 1, 1, 1, 14, 7)
        );
        assert_eq!(c.not_stored[2], 1, "dependency");
        assert_eq!(c.not_stored[11], 1, "policy");
        assert_eq!(c.not_stored.iter().sum::<u64>(), 2);
    }
}
