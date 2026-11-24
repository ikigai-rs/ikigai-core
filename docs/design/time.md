# Time domains and waking

Status: proposal. This note specifies a direction and follow-up work; it changes no
runtime behavior. Evidence is pinned to core
[`1d11151`](https://github.com/ikigai-rs/ikigai-core/tree/1d111516aa7e390110a8ad1007e96576966eb90a)
and throttle [`476d546`](https://github.com/ikigai-rs/ikigai-throttle/tree/476d546dc6d04c31f3707ad8772484f72cddea39).

## Four different questions

| Domain | Question | Proposed source and use |
| --- | --- | --- |
| World instant | What time is the world being read as of? | The innermost temporal corridor's pinned instant; absent a corridor, the invocation's existing `now()` falls back to operational time. It affects answers and scope identity. |
| Operational time | What epoch time does this host use to interpret absolute deadlines and cache expiry? | The host's optional kernel `Clock`. A deadline and the clock used to measure it must share an epoch and interpretation. |
| Elapsed time | How much time passed for a window, cooldown, or local budget? | A host-supplied monotonic source, with an opaque local instant and duration arithmetic. It must not move backward. It need not track wall-clock corrections. |
| Waking | When can a suspended future be polled again? | A host-supplied timer paired with the elapsed source. Reading a clock alone cannot wake a future. |

`Time` is an epoch-millisecond value, not a monotonic instant. A wall clock can
jump in either direction; a monotonic clock anchored once to the wall is useful
for elapsed arithmetic, but gradually diverges from corrected wall time. Do not
silently use that synthetic epoch for peer deadlines, expiry, or historical reads.
Do not serialize monotonic instants or compare them across hosts or source instances.

## Evidence and present reachability

* Core's [`Clock` and `SystemClock`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/kernel.rs#L71-L98)
  only read time; `SystemClock` reads `SystemTime`. A kernel starts with no clock,
  and [`with_clock`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/kernel.rs#L2605-L2613)
  installs one. The kernel's [`Issuer::now` implementation](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/kernel.rs#L7410-L7412)
  reads it, but there is no inherent public `Kernel::now` or `clock` accessor.
* A [`Scope`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/space.rs#L140-L150)
  holds the corridor clock, not the kernel clock. Its [clock and now accessors](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/space.rs#L380-L390)
  return `None` without a corridor. [`Space::resolve`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/space.rs#L1456-L1458)
  receives a request and scope, not a kernel or invocation. An overlay cannot obtain
  operational time there from scope. Throttle instead [stores its own clock](https://github.com/ikigai-rs/ikigai-throttle/blob/476d546dc6d04c31f3707ad8772484f72cddea39/src/lib.rs#L369-L389).
* [`Invocation::now`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/endpoint.rs#L2251-L2260)
  selects an explicitly attached clock, then the corridor, then the issuer clock.
  [`time_left`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/endpoint.rs#L1249-L1253)
  subtracts that `now` from a deadline documented as being on the kernel clock
  ([implementation](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/endpoint.rs#L1282-L1289)).
  A pinned corridor can therefore lengthen or exhaust an operational deadline; without
  any clock, a stated deadline yields `None`. A temporary integration test issuing a
  deadline on a kernel without a clock failed an assertion that `time_left()` was
  `Some`, confirming the latter. `None` also means no deadline, so callers cannot
  distinguish the two through that method alone.
* Throttle's [native clock](https://github.com/ikigai-rs/ikigai-throttle/blob/476d546dc6d04c31f3707ad8772484f72cddea39/src/native_clock.rs#L1-L14)
  anchors wall time once and advances with `Instant`; its
  [timer](https://github.com/ikigai-rs/ikigai-throttle/blob/476d546dc6d04c31f3707ad8772484f72cddea39/src/timer.rs#L23-L58)
  sleeps a duration. Retry [checks its bounds and races work](https://github.com/ikigai-rs/ikigai-throttle/blob/476d546dc6d04c31f3707ad8772484f72cddea39/src/lib.rs#L873-L955)
  against that timer. Current throttle already [exports Timeout on wasm](https://github.com/ikigai-rs/ikigai-throttle/blob/476d546dc6d04c31f3707ad8772484f72cddea39/src/lib.rs#L152-L157):
  the [host must inject a timer](https://github.com/ikigai-rs/ikigai-throttle/blob/476d546dc6d04c31f3707ad8772484f72cddea39/src/timeout.rs#L1-L10)
  there. Thus the older claim that the overlay itself is gated off wasm is stale.

The wire deadline reconstruction claim has not been independently verified here: the
referenced wire source was not available at a public `ikigai-wire` repository. The
follow-up must inspect the actual door and its protocol before changing it.

**Inspected in #1916.** The door is the `ikigai-wire` crate inside `ikigai-rs/ikigai-cli`
(wire v9, cli PR 556), read at cli
[`7086789`](https://github.com/ikigai-rs/ikigai-cli/tree/70867899ec6545bed7b84b0faf3dcd5fff526011).
The protocol already carries a remaining duration, never an instant: `IssueWith`'s
`time_left_ms` is the caller's `Invocation::time_left`, rounded up to whole milliseconds
([`lib.rs` L325-L363](https://github.com/ikigai-rs/ikigai-cli/blob/70867899ec6545bed7b84b0faf3dcd5fff526011/crates/ikigai-wire/src/lib.rs#L325-L363)).
The claim holds on both ends. Every server rebuilds the deadline as
`IssueWith::issue_context(SystemClock.now())`
([ipc](https://github.com/ikigai-rs/ikigai-cli/blob/70867899ec6545bed7b84b0faf3dcd5fff526011/crates/ikigai-ipc/src/lib.rs#L2187),
[ipc stream](https://github.com/ikigai-rs/ikigai-cli/blob/70867899ec6545bed7b84b0faf3dcd5fff526011/crates/ikigai-ipc/src/stream.rs#L62),
[quic](https://github.com/ikigai-rs/ikigai-cli/blob/70867899ec6545bed7b84b0faf3dcd5fff526011/crates/ikigai-quic/src/lib.rs#L1374),
[quic stream](https://github.com/ikigai-rs/ikigai-cli/blob/70867899ec6545bed7b84b0faf3dcd5fff526011/crates/ikigai-quic/src/stream.rs#L143)),
so under a kernel on another clock the deadline lands on the wrong epoch, and on a
clockless kernel it is set but cannot be measured. The client sends `time_left`, which was
corridor-sensitive and is `None` on a clockless kernel, so a bounded run there crossed as
an unbounded one. Core now gives the door what it needs: `Kernel::deadline_in(left)` (the
receiving kernel's clock, `Unsupported { verb: None, .. }` without one) on the server, and
`Invocation::time_left_checked()` (refuses a deadline with no clock) on the client.
Transit time is not subtracted, since the protocol cannot measure it, so it extends the
budget by the one-way latency; `IssueContext::with_not_after` only shortens, so any local
deadline is kept. The cli change belongs to its own pull request.

## Proposed contract

Expose `Kernel::now() -> Option<Time>` and, if sharing the source is needed,
`Kernel::clock() -> Option<&Arc<dyn Clock>>`. Both report the configured operational
clock and never fall back to the system or a corridor. Give `Invocation` a distinct
`operational_now()` through its issuer, including an explicit detached test seam;
retain `now()` and its current precedence for world reads. Define the precedence of a
detached operational override separately from the existing world-clock override.
Change `time_left()` to use operational time in a deliberate compatibility release;
keep `None` for no deadline or unavailable clock, and offer a distinguishable result
for code that must enforce a deadline. An attached override must not accidentally
change a kernel-issued run's operational deadline. Cache expiry remains on the kernel
clock; corridor identity and world reads remain unchanged.

Add an explicit resolution-services context to `Space::resolve` in a separately
planned API migration, carrying optional operational and elapsed services, not as part
of `Scope`'s corridor fingerprint or authority. Existing overlay builders can continue
to supply their own services until migrated. A resolve-time consumer reads the elapsed
source from that context; a wrapped endpoint receives the same services in its
invocation. Explicit overlay injection should win over inherited services. A missing
service stays missing: never install a platform clock implicitly. Preserve services
through nested resolution and sub-requests; avoid adding mutable clock readings to
cache keys. A clock-dependent admission decision must still execute on cache hits or
be documented as applying only to resolution; that behavior needs an explicit test.

Introduce a distinct `MonotonicClock` with opaque instants tied to one source and
checked/saturating duration arithmetic, plus an object-safe `Timer` with
`wait_until(instant)` returning a boxed `Send` future. The host installs a coherent
pair as one service, so a timer never interprets an instant from another source.
Waits at or before now complete promptly; later waits do not complete early, may
complete late, and are canceled by dropping the future. An unrepresentable deadline
must return an error rather than wrap. Specify equality at the deadline and executor
polling order for races; cancellation cannot undo external side effects. Implementations
must not block the executor. A deterministic fake advances the clock and wakes all
due waiters; dropping one removes its registration. A native adapter may use `Instant`
and a runtime-independent timer; a wasm host supplies an adapter, potentially bridging
local JavaScript callbacks to a `Send` future. Neither is a new default in this note.

For relative waits, take a monotonic reading once and checked-add the duration; use
the same timer for Retry backoff, RetryAfter and Timeout, including streamed bodies.
RetryAfter is a minimum: no early wake, truncation, or retry if its delay cannot fit
the remaining budget. Recheck bounds after a late wake and before reissuing. Race
work against the earliest applicable timeout; dropping work does not guarantee a
remote mutation was canceled. Synchronous blocking work cannot be interrupted by
an async timer. With a missing clock or timer, fail before starting bounded work;
use [`Unsupported { verb: None }`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/error.rs#L345-L363)
for a missing host facility, retaining useful detail. This changes the existing
throttle error class and must be called out in its release.

A wire door must reconstruct an absolute peer deadline in the receiving kernel's
operational clock domain, never in corridor time or a hidden system clock. Prefer
transmitting a remaining duration and subtracting transit time when the protocol can
measure it; otherwise document that transit and clock skew can extend or shorten the
budget. Clamp to any local deadline and never extend it. Missing time must be an
explicit refusal for a bounded request, not silently an unbounded run. Absolute epoch
deadlines across hosts require a stated clock-skew policy; local monotonic instants
must never cross the wire.

## Proposed follow-up items

1. **Expose operational time and repair deadline accounting** (#1916). Add the
   kernel and invocation accessors, distinguish a missing clock from no deadline,
   and measure `time_left` on operational time. Test absent, fixed, moving, detached,
   and pinned-past/future clocks. Inspect the real wire door and protocol, then test
   reconstruction and missing-clock behavior with the receiving kernel's clock.
2. **Provide a monotonic elapsed-time source** (#1560). Define source-bound instants,
   arithmetic and native/host adapters; migrate windows and cooldowns without treating
   their values as epoch timestamps. Test clock corrections, overflow and source mixups.
3. **Make operational services available during resolution** (#1559). Design the
   `Space::resolve` migration, propagate services independently of corridor identity,
   and verify a rate window slides under a pinned corridor and across cache paths.
4. **Use a host timer for wasm Timeout** (#1561). Migrate the existing wasm-capable
   overlay to the shared service; test host injection, absent timer refusal, racing,
   cancellation and streaming on wasm. Do not claim current Timeout is wasm-gated.
5. **Share a timer for Retry and Timeout** (#1900). Define the paired wait-until
   contract and deterministic fake, migrate RetryAfter and backoff, and classify
   absent clock/timer refusals as unsupported facilities. Test deadlines, late wakes,
   cancellation, no early retry and no implicit platform defaults.
