# Endpoints that read the config home — design

**Status:** settled 2026-08-23. An endpoint takes its config home **at construction**,
never at the point of use. Core supplies the path algebra and will not supply a handle
to hold it. Companion to `ikigai_core::config` (PRs #86, #87, #90) and to the
`Invocation` clock seam that landed alongside this note. This document is the
rationale; the code is the spec where they differ.

Sibling to `ambient-app-name.md`, and the same shape of question one level along: that
one asked who supplies the app NAME, this one asks who holds the config HOME and when.

---

## The problem

`config::config_home()` reads `$XDG_CONFIG_HOME` and `$HOME`. Both are process-global.
So an endpoint whose body calls it:

- reads the **developer's real** `~/.config/ikigai` under `cargo test` — on the machine
  this was written on, a real `a11y.toml` with a real contrast floor in it;
- cannot be given a different one by a test without `set_var`, which races the
  harness's own threads;
- gives two tests in one binary no way to disagree, because there is one environment
  and `cargo test` shares a process across a crate's tests.

None of that is a core defect. `config` already ships the injected form of every rule
it states — `config_home_from`, `layered_paths_in`, `data_home_in`, `data_path_in` —
and its own doc comments call those "the only testable form". The seam exists. The
question is who holds the value it produces, and the answer has to be made at a level
core cannot see.

## What it costs when the read stays ambient

`ikigai-a11y` is the case to look at, because it got the loader right and the endpoints
wrong, which is what makes it evidence rather than an oversight. `load::complete_in(home,
app)` and `load::threads_in(home, app)` take the home; `endpoints.rs` calls the ambient
`complete(app)` / `threads(app)` instead.

Read its test module and the consequence is visible without running anything. Five
endpoint tests, and every one of them either bails —

```rust
let Ok(rep) = invoke(&presentation(), request, &nothing) else {
    return; // no config home on this machine; the loader said so
};
```

— or asserts only that a field has the right **type**: `is_string()`, `is_number()`,
`is_boolean()`, `is_null()`. Not one pins a value, because not one owns the file the
value comes from. On a machine with no `HOME` the tests silently assert nothing at all;
on this machine they assert against a file that is not theirs.

That is the shape to recognize: tests written *around* a read rather than *of* it. They
pass, they stay passing, and they are not evidence about the endpoint.

## The shape: the handle takes its home at construction

`ikigai-log` is the reference implementation.

```rust
// The honest form: the caller states the home. A test hands it a tempdir.
pub fn new(home: Option<PathBuf>, app: Option<String>, config: LogConfig) -> LogHandle;

// The sugar, for a host configuring itself from the machine it is running on.
pub fn ambient(app: Option<String>, base: LogConfig) -> Result<LogHandle, ConfigError>;
```

The endpoints close over the handle, so the ambient read happens **once, in a host, at
startup**, where it is a fact about the process rather than a hidden input to a
resolution. The rules:

1. **The injected constructor is the real one; the ambient one is sugar over it.**
   `ambient()` resolves the home and calls `new()`. One code path, so the production
   path is the tested path with a different argument.
2. **`Option<PathBuf>`, not a fallible constructor.** No config home is a legal state —
   the process is under-configured, not broken — and it is `config_home()`'s own
   contract (`None` rather than a guess) carried up one level intact.
3. **Taken at construction, never per request.** A caller does not get to choose which
   config applies; the host does. This is also what keeps the endpoint cacheable on a
   stable key, and it is the same rule `ambient-app-name.md` §2 sets for `app` — the
   home and the name travel together and are held by the same object.
4. **Expose what was taken.** `LogHandle::home()` lets the endpoint body — and a test —
   see the home it is working against instead of re-deriving it.
5. **Never guessed.** No `current_dir()` fallback, no `env!("CARGO_MANIFEST_DIR")`. A
   guessed home reads as success and is the failure `config_home()` refuses by design.

A test then builds the handle over a `tempdir`, writes the exact layers it wants to
assert about, and asserts values — with no environment variable in sight and no race
with the test beside it.

## Why this is documentation and not a core helper

Core cannot own the handle. The handle holds the module's *config*, parsed by the
module, in the module's own type; `LogHandle` carries a `LogConfig` and an app name,
and the equivalent for `ikigai-a11y` carries an `A11y`. A core type generic enough to
hold all of them would hold nothing, and would still leave every module to write the
constructor pair that is the actual pattern.

What core *can* own, it already owns. The `_in` / `_from` functions are the seam, they
are public, and they are documented as the testable form. Nothing is missing from the
code. What was missing is that the next module author reads `config_home()`, sees a
function that works, and calls it from an endpoint body — because nothing at the point
of the decision says not to. So the fix goes where the question occurs: a pointer from
`config`'s module doc to here.

This is the same conclusion `ambient-app-name.md` reached about `app`, and for the same
reason: the value is a property of the **mount**, not of the process, and one process
legitimately has two answers. A test binary that mounts a fixture home beside a mount
with none is that case, and it is the ordinary case, not a corner.

## What core did change, next door

The `Invocation` clock seam that landed with this note is the same problem in the other
ambient input, resolved the other way — and the difference is instructive. "Now" is a
value core already models (`Clock`, injected into the `Kernel`), so core could close it
in code: `Invocation::with_clock`, plus a shipped `FixedClock`. The config home is a
value core models only as *path algebra*; the holding belongs above. Same disease, two
different right answers, because one of them has a core-shaped seam and the other does
not.

## What would reopen this

Not a preference for less boilerplate. The bar is `ambient-app-name.md`'s — a count
**plus** a demonstrated failure:

- **three or more modules** having independently written the `new(home, …)` /
  `ambient(…)` constructor pair, **and** an observed divergence between two of them
  that a shared helper would have prevented — one treating an absent home as an error
  where another treats it as a legal state, say, or two disagreeing about whether the
  ambient sugar re-reads the environment per call.

Two modules writing the same small constructor pair correctly is the pattern working.

A related count is already accumulating next door and deliberately not acted on, as the
worked example of this bar: **three** crates (`ikigai-core`'s own tests, `ikigai-http`,
`ikigai-cms-web`) carry an `AtomicU64`-backed settable test clock, which is one more
copy than the fixed clock that just got pushed down. It stays out of core because all
three are correct, none has drifted, and no behaviour goes untested because of them —
count without drift, which is exactly the case this section says is not enough.
