//! What `urn:kernel:constraint`'s window costs a resolution today, and what the cumulative
//! totals proposed in `docs/design/constraint-history.md` (option (b)) would add.
//!
//! Part 1 times whole resolutions through a real `Kernel` (the window is always on, so it is
//! inside every number); part 1b repeats two of them with a host `Tracer` folding every event
//! into totals, the only way a host can keep totals today. Part 2 times the window's own bookkeeping in isolation: a copy of
//! `record_resolution` (one lock, one `String` of the target, one push and pop on a
//! `VecDeque`) beside a copy-free stand-in for the proposed bounded top-K totals, alone and
//! under 16-thread contention. Part 3 times a read of `urn:kernel:constraint` itself.
//!
//! Numbers vary by machine; the design note quotes the run it was written from.

use std::collections::{HashMap, VecDeque};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::executor::block_on;
use ikigai_core::{
    Capability, Clock, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation,
    Request, Time, TraceEvent, Tracer, UriTemplate, Verb,
};

/// The host-only way to cumulative totals today: a global `Tracer` that folds each event
/// into per-target totals. It sees every resolution, at the price of an event per one.
#[derive(Default)]
struct Aggregating(Mutex<HashMap<String, (u64, u64, u64)>>);

impl Tracer for Aggregating {
    fn record(&self, event: TraceEvent) {
        let ms = match (event.started, event.ended) {
            (Some(s), Some(e)) => e.as_millis().saturating_sub(s.as_millis()),
            _ => 0,
        };
        let mut rows = self.0.lock().unwrap();
        let row = rows.entry(event.target).or_default();
        if event.cache_hit {
            row.2 += 1;
        } else {
            row.0 += ms;
            row.1 += 1;
        }
    }
}

struct SteppingClock(AtomicU64);

impl Clock for SteppingClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

fn body(text: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), text.as_bytes().to_vec())
}

fn kernel() -> Kernel {
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:measure:volatile"),
            FnEndpoint::new("volatile", |_| Ok(body("v"))),
        )
        .bind(
            Exact::new("urn:measure:kept"),
            FnEndpoint::new("kept", |_| Ok(body("k").cacheable())),
        )
        .bind(
            Exact::new("urn:measure:failing"),
            FnEndpoint::new("failing", |_| {
                Err(ikigai_core::Error::Endpoint("measured failure".to_string()))
            }),
        )
        .bind(
            UriTemplate::parse("urn:measure:item:{n}").unwrap(),
            FnEndpoint::new("item", |_| Ok(body("i"))),
        );
    Kernel::new(Arc::new(space)).with_clock(Arc::new(SteppingClock(AtomicU64::new(1))))
}

fn per_op(label: &str, n: u64, f: impl FnMut(u64)) -> f64 {
    let mut f = f;
    for i in 0..n / 10 {
        f(i);
    }
    let start = Instant::now();
    for i in 0..n {
        f(i);
    }
    let ns = start.elapsed().as_nanos() as f64 / n as f64;
    println!("  {label:<58} {ns:>9.1} ns");
    ns
}

// ---- Part 2: the window's bookkeeping, copied from kernel.rs `record_resolution` ----

#[allow(dead_code)]
struct Sample {
    target: String,
    elapsed_ms: Option<u64>,
    cache_hit: bool,
}

const WINDOW: usize = 512;

/// The sample as ledger #4095 records it: the end stamp beside the elapsed time, from the
/// same clock read, and an outcome in place of the hit flag.
#[allow(dead_code)]
struct StampedSample {
    target: String,
    timing: Option<(u64, u64)>,
    outcome: u8,
}

fn record_stamped(
    window: &Mutex<VecDeque<StampedSample>>,
    target: &str,
    now: u64,
    ms: u64,
    outcome: u8,
) {
    let mut window = window.lock().unwrap();
    if window.len() >= WINDOW {
        window.pop_front();
    }
    window.push_back(StampedSample {
        target: target.to_string(),
        timing: Some((now, ms)),
        outcome,
    });
}

fn record_window(window: &Mutex<VecDeque<Sample>>, target: &str, ms: u64, hit: bool) {
    let mut window = window.lock().unwrap();
    if window.len() >= WINDOW {
        window.pop_front();
    }
    window.push_back(Sample {
        target: target.to_string(),
        elapsed_ms: Some(ms),
        cache_hit: hit,
    });
}

/// A stand-in for option (b): per-target cumulative (uncached ms, uncached count, cached
/// count), bounded to `TOP_K` names with a Space-Saving replacement of the lightest entry
/// when a new name arrives at a full table. A name already present costs a hash lookup and
/// three adds, no allocation.
const TOP_K: usize = 256;

#[derive(Default)]
struct Totals {
    rows: HashMap<String, (u64, u64, u64)>,
}

fn record_totals(totals: &Mutex<Totals>, target: &str, ms: u64, hit: bool) {
    let mut totals = totals.lock().unwrap();
    if let Some(row) = totals.rows.get_mut(target) {
        if hit {
            row.2 += 1;
        } else {
            row.0 += ms;
            row.1 += 1;
        }
        return;
    }
    if totals.rows.len() >= TOP_K {
        // Space-Saving: evict the lightest, and let the newcomer inherit its weight so the
        // table's totals overestimate, never underestimate, a heavy hitter.
        let (lightest, inherited) = totals
            .rows
            .iter()
            .min_by_key(|(_, row)| row.0)
            .map(|(name, row)| (name.clone(), *row))
            .unwrap();
        totals.rows.remove(&lightest);
        let mut row = inherited;
        if hit {
            row.2 += 1;
        } else {
            row.0 += ms;
            row.1 += 1;
        }
        totals.rows.insert(target.to_string(), row);
        return;
    }
    totals
        .rows
        .insert(target.to_string(), if hit { (0, 0, 1) } else { (ms, 1, 0) });
}

fn contended(
    label: &str,
    threads: usize,
    per_thread: u64,
    f: impl Fn(u64) + Send + Sync + 'static,
) {
    let f = Arc::new(f);
    let start = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let f = Arc::clone(&f);
            std::thread::spawn(move || {
                for i in 0..per_thread {
                    f(i * threads as u64 + t as u64);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let total = threads as u64 * per_thread;
    let ns = start.elapsed().as_nanos() as f64 / total as f64;
    println!("  {label:<58} {ns:>9.1} ns (wall per op, {threads} threads)");
}

fn main() {
    let root = Capability::root();
    let n = 200_000;

    println!("part 1: whole resolutions through a Kernel (window always on)");
    let k = kernel();
    let volatile = Request::new(Verb::Source, Iri::parse("urn:measure:volatile").unwrap());
    per_op("issue, volatile endpoint (computed every time)", n, |_| {
        black_box(block_on(k.issue(volatile.clone(), &root)).unwrap());
    });
    let kept = Request::new(Verb::Source, Iri::parse("urn:measure:kept").unwrap());
    per_op("issue, cacheable endpoint (cache hit)", n, |_| {
        black_box(block_on(k.issue(kept.clone(), &root)).unwrap());
    });
    let failing = Request::new(Verb::Source, Iri::parse("urn:measure:failing").unwrap());
    per_op("issue, failing endpoint (fails every time)", n, |_| {
        black_box(block_on(k.issue(failing.clone(), &root)).unwrap_err());
    });
    let items: Vec<Request> = (0..1024)
        .map(|i| {
            Request::new(
                Verb::Source,
                Iri::parse(format!("urn:measure:item:{i}")).unwrap(),
            )
        })
        .collect();
    per_op("issue, template endpoint, 1024 distinct names", n, |i| {
        black_box(block_on(k.issue(items[(i % 1024) as usize].clone(), &root)).unwrap());
    });

    println!("part 1b: the same, with a host Tracer aggregating totals (the host-only (b))");
    let traced = kernel();
    let aggregate = Arc::new(Aggregating::default());
    traced.set_tracer(aggregate.clone());
    per_op(
        "issue, volatile endpoint, aggregating tracer installed",
        n,
        |_| {
            black_box(block_on(traced.issue(volatile.clone(), &root)).unwrap());
        },
    );
    per_op(
        "issue, cacheable endpoint, aggregating tracer installed",
        n,
        |_| {
            black_box(block_on(traced.issue(kept.clone(), &root)).unwrap());
        },
    );

    println!("part 2: the bookkeeping alone, one thread");
    let names: Vec<String> = (0..1024).map(|i| format!("urn:measure:item:{i}")).collect();
    let few: Vec<String> = (0..32).map(|i| format!("urn:measure:item:{i}")).collect();
    let window = Mutex::new(VecDeque::with_capacity(WINDOW));
    per_op("window: record_resolution copy (today)", n, |i| {
        record_window(&window, &few[(i % 32) as usize], i % 7, i % 3 == 0)
    });
    let stamped = Mutex::new(VecDeque::with_capacity(WINDOW));
    per_op("window: stamped sample with outcome (#4095)", n, |i| {
        record_stamped(&stamped, &few[(i % 32) as usize], i, i % 7, (i % 3) as u8)
    });
    let totals = Mutex::new(Totals::default());
    per_op("totals (b): 32 names, all resident", n, |i| {
        record_totals(&totals, &few[(i % 32) as usize], i % 7, i % 3 == 0)
    });
    let totals = Mutex::new(Totals::default());
    per_op(
        "totals (b): 1024 names cycling through K=256 (worst case)",
        n,
        |i| record_totals(&totals, &names[(i % 1024) as usize], i % 7, i % 3 == 0),
    );

    println!("part 2b: the bookkeeping under contention");
    let window = Arc::new(Mutex::new(VecDeque::with_capacity(WINDOW)));
    let few = Arc::new(few);
    {
        let (w, f) = (Arc::clone(&window), Arc::clone(&few));
        contended(
            "window: record_resolution copy (today)",
            16,
            50_000,
            move |i| record_window(&w, &f[(i % 32) as usize], i % 7, i % 3 == 0),
        );
    }
    {
        let stamped = Arc::new(Mutex::new(VecDeque::with_capacity(WINDOW)));
        let f = Arc::clone(&few);
        contended(
            "window: stamped sample with outcome (#4095)",
            16,
            50_000,
            move |i| record_stamped(&stamped, &f[(i % 32) as usize], i, i % 7, (i % 3) as u8),
        );
    }
    let totals = Arc::new(Mutex::new(Totals::default()));
    {
        let (t, f) = (Arc::clone(&totals), Arc::clone(&few));
        contended(
            "window + totals (b), 32 resident names",
            16,
            50_000,
            move |i| {
                record_window(&window, &f[(i % 32) as usize], i % 7, i % 3 == 0);
                record_totals(&t, &f[(i % 32) as usize], i % 7, i % 3 == 0)
            },
        );
    }

    println!("part 3: reading urn:kernel:constraint (512 samples held)");
    let read = Request::new(Verb::Source, Iri::parse("urn:kernel:constraint").unwrap());
    per_op("issue urn:kernel:constraint, text face", 20_000, |_| {
        black_box(block_on(k.issue(read.clone(), &root)).unwrap());
    });
    let text = block_on(k.issue(read.clone(), &root)).unwrap();
    println!("\n{}", String::from_utf8_lossy(&text.bytes));
}
