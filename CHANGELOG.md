# Changelog

## Unreleased: needs a minor bump (0.4.0)

This minor release carries two changes: the nested confinement below (ledger #3825), and the
missing scopes on a capability-floor refusal (ledger #3840).

### Added

- **A declared-floor refusal carries the scopes the caller lacked** (ledger #3840).
  `Error::lacking()` returns every scope an endpoint declares for the verb that the
  capability does not satisfy, in declaration order and spelled as declared (a family keeps
  its trailing `*`), when the kernel's floor refused the request before dispatch. It is
  empty for every other error, including a `Denied` an endpoint raises itself at run time. A
  host's door reads it to log which capability a refused request needed (an access log's
  `needs=` column) without parsing the sentence; a nested sub-request's floor refusal
  carries its own scope, and an endpoint that propagates it with `?` passes it on. The
  sentence is unchanged byte for byte and still names the first missing scope, as does the
  trace's `denied` note. The kind (`denied`), class (`Denied`), wire tag (4) and HTTP status
  (403) are unchanged. The JSON face (`Error::to_json`, `Error::from_json`) carries the
  list as `lacking` when it is not empty. The shared failure-class vectors gain a
  `denied-by-the-floor` row that pins it, and the plain `denied` row is unchanged.

### Changed (breaking)

- **`Error::Denied` holds a `Denial`, not a `String`** (ledger #3840). A `Denial` is the
  message and the missing scopes. It dereferences to `str` and displays as its message, so
  matching `Error::Denied(m)` and reading `m` (`m.contains(…)`, `format!("{m}")`) still
  compiles, and `Error::Denied("…".into())` still builds one. Building one from a `String`
  needs `.into()`: `Error::Denied(format!(…).into())`. Where a `String` is needed, use
  `denial.into_message()` or `denial.to_string()`. To reword a refusal and keep its scopes,
  use `Denial::reworded`.

### Changed (behavior)

- **A confinement inside a confinement drops the outer confined corridor** (ledger #3825).
  `Scope::confined`, `Invocation::confine` and `Confine` keep only the host's corridors:
  confining removes every earlier confinement, then puts the new corridor behind the host's
  corridors and severs the root. Through 0.3.5 a nested confinement ran in
  `⟨host corridors…, T₁, T₂⟩`: the inner endpoint could still reach the outer confined
  corridor `T₁`, and `T₁`, consulted first, shadowed `T₂`'s own door for a shared name. Now it
  runs in `⟨host corridors…, T₂⟩`, exactly the chain a fresh confinement builds. `Scope::stack`
  follows the same rule: an inner chain's confinement replaces the outer one's. This only
  narrows: dropping `T₁` removes reach and grants nothing. The chain fingerprint follows the
  chain, so a result cached in the old nested chain is not reused.
