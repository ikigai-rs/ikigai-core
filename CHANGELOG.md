# Changelog

## Unreleased: needs a minor bump (0.4.0)

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
